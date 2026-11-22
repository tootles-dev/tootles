#!/usr/bin/env python3
"""Offline PTY regression for Tootles terminal playback.

Generates synthetic media; launches a shell wrapper that makes a real application
exit distinguishable from merely exposing the old primary screen. Captures raw
output privately and reports only counts/terminal flags. No user media/network.

python3 scripts/test-terminal-playback.py --binary /path/to/tootles --expect-fixed
--expect-fixed makes regressions fail the process. Default records the baseline.
--case can select real-q, real-ctrl-c, real-eof, resize-eof, missing,
dirty-player, player-failure, or echo-exit.
The default exercises Alacritty-style automatic capability detection. Optional
--image-protocol halfblocks verifies the forced fallback independently.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

CSI = re.compile(rb"\x1b\[([0-?]*)([ -/]*)([@-~])")
FAKE = '#!' + sys.executable + '''
import os, sys, termios, time
print("TOOTLES_FAKE_PLAYER_ENTER", flush=True)
# Simulate a child that adjusts tty modes and fails to restore them exactly.
a = termios.tcgetattr(0)
a[0] ^= termios.IXON
a[1] ^= termios.OPOST
a[3] |= termios.ICANON | termios.ECHO
termios.tcsetattr(0, termios.TCSANOW, a)
print("TOOTLES_FAKE_PLAYER_DONE", flush=True)
time.sleep(0.1)
# Leave a synchronized frame, application keypad, mouse capture, and partial
# color command active, as an interrupted terminal renderer could do.
sys.stdout.write("\\x1b[?2026h\\x1b=\\x1b[?1003h\\x1b[38;2;1;")
sys.stdout.flush()
print("TOOTLES_FAKE_DIAGNOSTIC", file=sys.stderr, flush=True)
sys.exit(int(os.environ.get("TOOTLES_FAKE_STATUS", "0")))
'''

ECHO_PLAYER = '#!' + sys.executable + '''
import os, termios, time, tty
inherited = termios.tcgetattr(0)
tty.setraw(0)
os.write(1, b"\\x1b[?1003hTOOTLES_ECHO_PLAYER_RUNNING\\r\\n")
time.sleep(0.05)
# mpv restores the inherited tty state before it returns. Terminal mouse
# reports already in flight can arrive in the gap before the parent resumes.
termios.tcsetattr(0, termios.TCSANOW, inherited)
os.write(1, b"TOOTLES_ECHO_RESTORE_GAP\\r\\n")
time.sleep(0.3)
os.write(1, b"\\x1b[?1003l")
'''

class Session:
    def __init__(self, binary, config, env, log, image_protocol, mouse):
        read_gate, write_gate = os.pipe()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.close(write_gate)
            os.read(read_gate, 1)
            os.close(read_gate)
            # Keep a shell around to prove whether playback actually exits the
            # client, and put recognizable content on the primary screen.
            env = dict(env, TOOTLES_TEST_BINARY=str(binary), TOOTLES_TEST_CONFIG=str(config), TOOTLES_TEST_PROTOCOL=image_protocol, TOOTLES_TEST_MOUSE="yes" if mouse else "no")
            os.execve('/bin/sh', ['/bin/sh', '-c',
                'printf "TOOTLES_PRIMARY_SHELL_PROMPT$ \\n"; '
                'if [ "$TOOTLES_TEST_MOUSE" = no ]; then set -- --no-mouse; else set --; fi; '
                '"$TOOTLES_TEST_BINARY" "$@" --demo --no-motion --image-protocol "$TOOTLES_TEST_PROTOCOL" '
                '--config-dir "$TOOTLES_TEST_CONFIG"; '
                'code=$?; printf "TOOTLES_SHELL_RETURN:%s\\n" "$code"; exit "$code"'], env)
        os.close(read_gate)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 110, 990, 684))
        self.original_attrs = termios.tcgetattr(self.fd)
        os.write(write_gate, b'1')
        os.close(write_gate)
        self.output = bytearray()
        self.log = log
        self.status = None
        self.scan = b''
        self.alt = False
        self.mode_events = []
        self.private_modes = {}
        self.frames = []
        self.replies = 0
        self.echo_exit_probe = False
        self.echo_probe_injected = False

    def _receive(self, data):
        origin = len(self.output)
        self.output.extend(data)
        if (self.echo_exit_probe and not self.echo_probe_injected
                and b'TOOTLES_ECHO_RESTORE_GAP' in self.output):
            os.write(self.fd, b'\x1b[<35;12;9M' * 8)
            self.echo_probe_injected = True
        # Retain incomplete escape sequences across OS reads. This is a probe,
        # not a terminal renderer; only state changes and device queries matter.
        stream = self.scan + data
        base = origin - len(self.scan)
        last = 0
        for match in CSI.finditer(stream):
            params, intermediate, final = match.groups()
            position = base + match.start()
            if params.startswith(b'?') and final in (b'h', b'l'):
                for mode in params[1:].split(b';'):
                    if mode.isdigit(): self.private_modes[int(mode)] = final == b'h'
            if params in (b'?1049', b'?1047', b'?47') and final in (b'h', b'l'):
                self.alt = final == b'h'
                self.mode_events.append((position, self.alt, params.decode()))
            if final == b'm' and (b'48;2' in params or b'38;2' in params):
                self.frames.append((position, self.alt))
            reply = None
            if params == b'6' and final == b'n': reply = b'\x1b[1;1R'
            if params in (b'', b'0') and final == b'c': reply = b'\x1b[?1;2c'
            if params == b'16' and final == b't': reply = b'\x1b[6;19;9t'
            if params == b'5' and final == b'n': reply = b'\x1b[0n'
            if reply:
                os.write(self.fd, reply)
                self.replies += 1
            last = match.end()
        # Alacritty does not answer Kitty graphics probes; do not fake support.
        incomplete = stream.rfind(b'\x1b', last)
        self.scan = stream[incomplete:] if incomplete >= last else b''
        if len(self.scan) > 4096:
            self.scan = b''

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.fd], [], [], max(0, deadline-time.monotonic()))
            if not ready: break
            try: data = os.read(self.fd, 131072)
            except OSError: break
            if not data: break
            self._receive(data)

    def send(self, data, seconds=.18):
        os.write(self.fd, data)
        self.pump(seconds)

    def wait_text(self, text, seconds=8, start=0):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if text in self.output[start:]: return True
            self.pump(.1)
        return text in self.output[start:]

    def live(self):
        if self.status is not None: return False
        pid, status = os.waitpid(self.pid, os.WNOHANG)
        if pid:
            self.status = status
            return False
        return True

    def close(self):
        self.log.write_bytes(self.output)
        if self.live():
            # All descendants share this isolated test session/process group.
            try: os.killpg(self.pid, signal.SIGTERM)
            except ProcessLookupError: pass
            self.pump(.1)
            if self.live():
                try: os.killpg(self.pid, signal.SIGKILL)
                except ProcessLookupError: pass
                os.waitpid(self.pid, 0)
        os.close(self.fd)


def scenario(binary, directory, case, image_protocol, mouse):
    env = dict(os.environ, TERM='alacritty', COLORTERM='truecolor')
    for key in ('NO_COLOR', 'KITTY_WINDOW_ID', 'TERM_PROGRAM', 'TMUX', 'STY',
                'ITERM_SESSION_ID', 'LC_TERMINAL', 'WEZTERM_EXECUTABLE', 'KONSOLE_VERSION'):
        env.pop(key, None)
    if case == 'missing':
        env['PATH'] = str(directory/'missing-programs')
    elif case in ('dirty-player', 'player-failure', 'echo-exit'):
        programs = directory/'fake-programs'
        programs.mkdir(exist_ok=True)
        player = programs/'mpv'
        player.write_text(ECHO_PLAYER if case=='echo-exit' else FAKE)
        player.chmod(0o700)
        env['PATH'] = str(programs)
        env['TOOTLES_FAKE_STATUS'] = '13' if case=='player-failure' else '0'
    clip_dir = directory / ('short' if case in ('real-eof', 'resize-eof') else 'long')
    session = Session(binary, directory/('config-'+case), env, directory/(case+'.pty'), image_protocol, mouse)
    session.echo_exit_probe = case=='echo-exit'
    result = {'case':case, 'binary':str(binary), 'image_protocol':image_protocol, 'mouse':mouse}
    try:
        assert session.wait_text(b'tootles'), 'client did not open'
        session.send(b'n')
        session.send(b'\x1b[13~')
        session.send(b'\x0c\x15')
        session.send(b'\x1b[200~'+os.fsencode(clip_dir)+b'\x1b[201~\r', .4)
        session.send(b'p', .3)
        assert b'LOCAL MEDIA' in session.output, 'local video gallery did not open'
        session.send(b'v', .25)
        assert b'Terminal mode' in session.output, 'terminal playback mode not selected'
        start = len(session.output)
        session.send(b'\r', .5)
        if case in ('real-q', 'real-ctrl-c'):
            session.pump(.5)
            # mpv must own q, without exiting the gallery/client.
            session.send(b'\x03' if case=='real-ctrl-c' else b'q', .7)
        if case == 'resize-eof':
            fcntl.ioctl(session.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 86, 774, 532))
            session.pump(.2)
        expected = (b'mpv is unavailable' if case=='missing' else
                    b'mpv exited with status 13' if case=='player-failure' else
                    b'mpv exited with status 4' if case=='real-ctrl-c' else b'Playback finished')
        resumed = session.wait_text(expected, 8, start)
        segment = bytes(session.output[start:])
        banner = session.output.find('Tootles · Space pause'.encode(), start)
        alt_at_banner = False
        for position, enabled, _ in session.mode_events:
            if position > banner: break
            alt_at_banner = enabled
        attrs = termios.tcgetattr(session.fd)
        result.update(
            resumed=resumed,
            banner_found=banner>=start,
            banner_in_alternate_screen=alt_at_banner,
            left_alternate_before_player=sum(not alt for pos,alt,_ in session.mode_events if start<=pos<banner),
            halfblock_pixels=segment.count('▄'.encode())+segment.count('▀'.encode()),
            literal_escaped_csi=segment.count(b'^[['),
            terminal_replies_sent=session.replies,
            echo_probe_injected=session.echo_probe_injected,
            fake_diagnostic_leaked=b'TOOTLES_FAKE_DIAGNOSTIC' in segment,
            resumed_mouse_modes=[mode for mode in (1000,1002,1003,1006,1015) if session.private_modes.get(mode,False)],
            resumed_bracketed_paste=session.private_modes.get(2004,False),
            resumed_sync_output=session.private_modes.get(2026,False),
            actual_shell_return=b'TOOTLES_SHELL_RETURN:' in segment,
            app_still_running=session.live(),
            left_alternate_screen=sum(not alt for pos,alt,_ in session.mode_events if pos>=start),
            entered_alternate_screen=sum(alt for pos,alt,_ in session.mode_events if pos>=start),
            truecolor_sequences=sum(1 for pos,alt in session.frames if pos>=start),
            truecolor_on_primary=sum(1 for pos,alt in session.frames if pos>=start and not alt),
            resumed_echo=bool(attrs[3]&termios.ECHO),
            resumed_canonical=bool(attrs[3]&termios.ICANON),
            resumed_signals=bool(attrs[3]&termios.ISIG),
        )
        # Type only after successful return. A raw-mode failure would echo this
        # sequence, close the wrong view, or make the client unresponsive.
        if resumed:
            session.send(b'\x1b', .15)
            session.send(b'\x1b', .15)
            session.send(b'\x1b', .15)
            session.send(b'q', .3)
            result['clean_exit'] = session.wait_text(b'TOOTLES_SHELL_RETURN:0', 4, start)
            final = termios.tcgetattr(session.fd)
            result['shell_echo_restored'] = bool(final[3]&termios.ECHO)
            result['shell_canonical_restored'] = bool(final[3]&termios.ICANON)
            result['shell_modes_exact'] = final == session.original_attrs
            result['shell_flag_delta'] = [old^new for old,new in zip(session.original_attrs[:4], final[:4])]
        else:
            result['clean_exit'] = False
        result['pass'] = all([
            result['resumed'], not result['actual_shell_return'], result['app_still_running'],
            result['banner_found'], result['banner_in_alternate_screen'],
            result['left_alternate_before_player'] == 0,
            result['truecolor_on_primary'] == 0,
            not result['resumed_echo'], not result['resumed_canonical'], not result['resumed_signals'],
            result['clean_exit'], result.get('shell_modes_exact',False),
            bool(result['resumed_mouse_modes']) == mouse,
            result['resumed_bracketed_paste'], not result['resumed_sync_output'],
            result['halfblock_pixels']>3000 if case.startswith('real-') or case=='resize-eof' else True,
            result['literal_escaped_csi']==0, not result['fake_diagnostic_leaked'],
            result['echo_probe_injected'] if case=='echo-exit' else True,
        ])
    except Exception as error:
        result.update(error=str(error), **{'pass':False})
    finally:
        session.close()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--case', action='append', choices=['real-q','real-ctrl-c','real-eof','resize-eof','missing','dirty-player','player-failure','echo-exit'])
    parser.add_argument('--output', type=Path)
    parser.add_argument('--image-protocol', choices=['auto','halfblocks'], default='auto')
    parser.add_argument('--expect-fixed', action='store_true')
    parser.add_argument('--no-mouse', action='store_true')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    ffmpeg = shutil.which('ffmpeg')
    if not ffmpeg:
        parser.error('Install ffmpeg to generate the synthetic video fixture')
    cases = args.case or ['real-q','real-ctrl-c','real-eof','resize-eof','missing','dirty-player','player-failure','echo-exit']
    if any(case.startswith('real-') or case=='resize-eof' for case in cases) and not shutil.which('mpv'):
        parser.error('Install mpv to exercise real terminal playback')
    directory = args.output or Path(tempfile.mkdtemp(prefix='tootles-player-regression-'))
    directory.mkdir(exist_ok=True)
    for name, duration in [('short','2.0'),('long','12')]:
        folder=directory/name
        folder.mkdir(exist_ok=True)
        subprocess.run([ffmpeg,'-y','-hide_banner','-loglevel','error','-f','lavfi','-i',
            'testsrc2=size=80x48:rate=8','-t',duration,'-c:v','libx264','-pix_fmt','yuv420p',
            str(folder/'synthetic.mp4')],check=True)
    results=[]
    for case in cases:
        result=scenario(binary,directory,case,args.image_protocol,not args.no_mouse)
        results.append(result)
        print(json.dumps(result,sort_keys=True),flush=True)
    (directory/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    print('Evidence:',directory)
    if args.expect_fixed and not all(result.get('pass') for result in results):
        raise SystemExit(1)

if __name__=='__main__':
    main()
