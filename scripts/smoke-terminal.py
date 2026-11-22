#!/usr/bin/env python3
"""Offline Linux/macOS PTY smoke test. Build with cargo build --release first."""
import os, pty, fcntl, termios, struct, select, time, tempfile, json
scratch=tempfile.mkdtemp(prefix='tootles-pty-')
root=os.path.join(scratch,'private')
open_log=os.path.join(scratch,'opened-path')
stubs=os.path.join(scratch,'bin')
os.mkdir(stubs)
for program in ('xdg-open','open'):
 path=os.path.join(stubs,program)
 with open(path,'w') as script:
  script.write("#!/bin/sh\nprintf '%s\\n' \"$1\" > \"$TOOTLES_TEST_OPEN_LOG\"\n")
 os.chmod(path,0o700)
env=dict(os.environ, TERM='xterm-256color', COLORTERM='truecolor')
env.pop('NO_COLOR',None)
env['PATH']=stubs+os.pathsep+env.get('PATH','')
env['TOOTLES_TEST_OPEN_LOG']=open_log
binary=os.environ.get('TOOTLES_TEST_BINARY',os.path.abspath('target/release/tootles'))
attachments=os.path.abspath(os.path.join(os.path.dirname(__file__),'..','assets'))
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
 send(b'L',.3)
 assert b'Links in this post' in output,'post link picker did not open'
 send(b'\r',.5)
 assert open(open_log).read().strip()=='https://joinmastodon.org/','post body link target was not opened'
 os.remove(open_log)
 send(b'\x1b',.3)
 send(b'n',.5)
 send('\x1b[200~Hello from the PTY smoke test.\nUnicode works: 🌌 café\x1b[201~'.encode())
 # Browse and preview a bundled image without opening a browser or network URL.
 send(b'\x1b[13~',.4)
 send(b'\x0c\x15')
 send(('\x1b[200~'+attachments+'\x1b[201~\r').encode(),.6)
 send(b'p',.8)
 assert b'LOCAL MEDIA' in output,'local image gallery did not open'
 assert '▀'.encode() in output or '▄'.encode() in output,'local image pixels were not rendered'
 assert b'Block preview' in output,'image quality fallback was not explained'
 send(b'v',.3)
 assert b'Default image viewer' in output,'native image view option unavailable'
 assert not os.path.exists(open_log),'mode switch unexpectedly launched an application'
 send(b'\r',1)
 opened_image=open(open_log).read().strip()
 assert opened_image.startswith(os.path.join(root,'image-previews')+os.sep),opened_image
 assert open(opened_image,'rb').read()==open(os.path.join(attachments,'demo-desktop.png'),'rb').read(),'original image bytes changed'
 os.remove(open_log)
 send(b'\x1b',.3)
 send(b'\r',.3)
 send(b'\x1b[200~A bundled landscape used to verify the attachment browser.\x1b[201~')
 send(b'\x1ba',.3)
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
 assert len(data[0]['attachments'])==1,data
 assert data[0]['attachments'][0]['path']==os.path.join(attachments,'demo-desktop.png'),data
 assert data[0]['attachments'][0]['description']=='A bundled landscape used to verify the attachment browser.',data
 print('PASS: real PTY launch, Unicode paste, post link picker/browser target, inline/native original image viewing, browse/attach with alt text, save/resume draft, search, mouse, resize, help, quit, terminal restoration')
 print('Captured',len(output),'terminal bytes; draft verified')
finally:
 try: os.kill(pid,15)
 except ProcessLookupError: pass
 os.close(fd)
