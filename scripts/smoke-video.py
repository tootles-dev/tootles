#!/usr/bin/env python3
"""Offline PTY check of real mpv handoff and missing-player recovery.

Requires ffmpeg and mpv. Generates a short silent test clip; no network or user
media is accessed. Build Tootles first or set TOOTLES_TEST_BINARY.
"""
import fcntl
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time

binary = os.environ.get("TOOTLES_TEST_BINARY", os.path.abspath("target/release/tootles"))
for tool in ("ffmpeg", "mpv"):
    if not shutil.which(tool):
        raise SystemExit(f"Install {tool} to run this optional video smoke test")


def scenario(directory, player_available):
    env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor")
    env.pop("NO_COLOR", None)
    if not player_available:
        env["PATH"] = os.path.join(directory, "no-programs")
    config = os.path.join(directory, "config-present" if player_available else "config-missing")
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(binary, [binary, "--demo", "--no-motion", "--image-protocol", "halfblocks", "--config-dir", config], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 110, 990, 684))
    output = bytearray()

    def drain(seconds):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            ready, _, _ = select.select([fd], [], [], max(0, until - time.monotonic()))
            if ready:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                output.extend(data)
                if b"\x1b[6n" in data:
                    os.write(fd, b"\x1b[1;1R")
                if b"\x1b[c" in data:
                    os.write(fd, b"\x1b[?1;2c")
                if b"\x1b[16t" in data:
                    os.write(fd, b"\x1b[6;19;9t")
                if b"\x1b[5n" in data:
                    os.write(fd, b"\x1b[0n")

    def send(data, delay=0.25):
        os.write(fd, data)
        drain(delay)

    try:
        drain(0.8)
        send(b"n")
        send(b"\x1b[13~")
        send(b"\x0c\x15")
        location = os.path.join(directory, "clips")
        send(("\x1b[200~" + location + "\x1b[201~\r").encode(), 0.5)
        send(b"p", 0.4)
        assert b"LOCAL MEDIA" in output, "video gallery did not open: " + re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", output).decode("utf-8", "replace")[-3000:]
        send(b"v")
        start = len(output)
        send(b"\r", 0.9)
        if player_available:
            assert b"\x1b[48;2" in output[start:], "real mpv produced no true-color video frames"
            send(b"q", 0.6)
        else:
            assert b"mpv is unavailable" in output[start:], "missing player error was not shown"
        # mpv's own terminal driver leaves its alternate screen when it exits.
        # Tootles must never expose the shell merely to launch a player; when
        # mpv is absent there should be no LeaveAlternateScreen at all.
        if player_available:
            assert output[start:].count(b"\x1b[?1049l") >= 1, "mpv did not complete screen cleanup"
        else:
            assert output[start:].count(b"\x1b[?1049l") == 0, "missing player exposed the shell screen"
        assert output[start:].count(b"\x1b[?1049h") >= 1, "TUI was not resumed"
        send(b"\x1b")  # Gallery -> browser.
        send(b"\x1b")  # Browser -> composer.
        send(b"\x1b")  # Composer -> timeline.
        send(b"q", 0.5)
        for _ in range(20):
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                break
            drain(0.1)
        else:
            raise AssertionError("TUI did not exit after video playback")
        assert os.waitstatus_to_exitcode(status) == 0
        flags = termios.tcgetattr(fd)[3]
        assert flags & termios.ICANON and flags & termios.ECHO, "shell terminal mode was not restored"
    finally:
        try:
            os.kill(pid, 15)
        except ProcessLookupError:
            pass
        os.close(fd)


with tempfile.TemporaryDirectory(prefix="tootles-video-") as directory:
    clips = os.path.join(directory, "clips")
    os.mkdir(clips)
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
        "testsrc2=size=80x48:rate=8", "-t", "3", "-c:v", "libx264", "-pix_fmt",
        "yuv420p", os.path.join(clips, "silent-test.mp4"),
    ], check=True)
    scenario(directory, True)
    scenario(directory, False)
print("PASS: real mpv terminal frames, q returns to Tootles, missing-player recovery, terminal restoration")
