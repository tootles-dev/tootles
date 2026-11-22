#!/usr/bin/env python3
"""Offline Linux/macOS PTY smoke test. Build with cargo build --release first."""
import os, pty, fcntl, termios, struct, select, time, tempfile, json
root=tempfile.mkdtemp(prefix='tootles-pty-')
root=os.path.join(root,'private')
env=dict(os.environ, TERM='xterm-256color', COLORTERM='truecolor')
env.pop('NO_COLOR',None)
binary=os.environ.get('TOOTLES_TEST_BINARY',os.path.abspath('target/release/tootles'))
pid,fd=pty.fork()
if pid==0:
 os.execve(binary,[binary,'--demo','--no-motion','--config-dir',root],env)
fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack('HHHH',44,144,1296,836))
output=bytearray()
def drain(seconds):
 until=time.monotonic()+seconds
 while time.monotonic()<until:
  ready,_,_=select.select([fd],[],[],max(0,until-time.monotonic()))
  if ready:
   try:data=os.read(fd,65536)
   except OSError:break
   if not data:break
   output.extend(data)
   if b"\x1b[6n" in data: os.write(fd,b"\x1b[1;1R")
   if b"\x1b[c" in data: os.write(fd,b"\x1b[?1;2c")
   if b"\x1b[16t" in data: os.write(fd,b"\x1b[6;19;9t")
   if b"\x1b[5n" in data: os.write(fd,b"\x1b[0n")
def send(data,wait=.2):
 os.write(fd,data);drain(wait)
try:
 drain(3)
 assert b'tootles' in output,output[-1000:]
 send(b'n',.5)
 send('\x1b[200~Hello from the PTY smoke test.\nUnicode works: 🌌 café\x1b[201~'.encode())
 send(b'\x13')
 send(b'\x1b',.3)
 send(b'd')
 send(b'\r')
 send(b'\x1b',.3)
 send(b'/rust\r',.4)
 send(b'\x1b[<0;10;7M\x1b[<0;10;7m',.3)
 fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack('HHHH',20,60,540,380))
 drain(.3)
 send(b'?',.2)
 send(b'\x1b',.3)
 send(b'q',.4)
 for _ in range(10):
  done,status=os.waitpid(pid,os.WNOHANG)
  if done:break
  drain(.2)
 else:
  os.kill(pid,9)
  raise AssertionError('TUI failed to exit')
 assert os.waitstatus_to_exitcode(status)==0,output[-1000:]
 assert b'\x1b[?1049l' in output,'alternate screen was not restored'
 drafts=[n for n in os.listdir(root) if n.startswith('drafts-')]
 assert drafts,os.listdir(root)
 data=json.load(open(os.path.join(root,drafts[0])))
 assert data[0]['text']=='Hello from the PTY smoke test.\nUnicode works: 🌌 café',data
 print('PASS: real PTY launch, bracketed multiline Unicode paste, save/resume draft, search, mouse input, resize, help, quit, terminal restoration')
 print('Captured',len(output),'terminal bytes; draft verified')
finally:
 try: os.kill(pid,15)
 except ProcessLookupError: pass
 os.close(fd)
