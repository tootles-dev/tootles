#!/usr/bin/env python3
"""Exercise two account types against a local server in a real Unix PTY.

No real credentials, remote requests, or social posts are used.
Build with cargo build --release first; override TOOTLES_TEST_BINARY if needed.
"""
import base64
import fcntl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time
import tomllib
from urllib.parse import urlparse

DID = "did:plc:abcdefghijklmnopqrstuvwx"
HANDLE = "smoke.bsky.social"
PASSWORD = "aaaa-bbbb-cccc-dddd"


def jwt(scope):
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")
    return encode({"alg": "HS256"}) + "." + encode({
        "sub": DID, "scope": scope, "exp": int(time.time()) + 3600,
    }) + ".test-signature"


ACCESS = jwt("com.atproto.access")
REFRESH = jwt("com.atproto.refresh")
AUTHOR = {"did": DID, "handle": HANDLE, "displayName": "Blue Smoke"}
POST = {
    "uri": f"at://{DID}/app.bsky.feed.post/3l3w2abc23422",
    "cid": "bafyreigfxvcsujsmoke",
    "author": AUTHOR,
    "record": {"$type": "app.bsky.feed.post", "text": "Bluesky smoke marker",
               "createdAt": "2026-11-22T18:30:00Z"},
    "indexedAt": "2026-11-22T18:30:00Z", "likeCount": 2, "repostCount": 0,
    "replyCount": 0, "viewer": {},
}
MASTODON_ACCOUNT = {"id": "42", "username": "smoke", "acct": "smoke",
                    "display_name": "Masto Smoke", "note": "", "url": "https://example.org/@smoke"}
REQUESTS = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, payload, status=200):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlparse(self.path).path
        REQUESTS.append(("GET", path, self.headers.get("Authorization")))
        if path == "/xrpc/com.atproto.server.getSession":
            self.respond({"did": DID, "handle": HANDLE, "active": True})
        elif path == "/xrpc/app.bsky.actor.getProfile":
            self.respond({**AUTHOR, "followersCount": 12, "followsCount": 8, "postsCount": 1, "viewer": {}})
        elif path == "/xrpc/app.bsky.feed.getTimeline":
            self.respond({"feed": [{"post": POST}]})
        elif path == "/api/v1/accounts/verify_credentials":
            self.respond(MASTODON_ACCOUNT)
        elif path == "/api/v1/timelines/home":
            self.respond([{"id": "101", "account": MASTODON_ACCOUNT,
                           "content": "<p>Mastodon smoke marker</p>",
                           "url": "https://example.org/@smoke/101",
                           "created_at": "2026-11-22T18:30:00Z", "visibility": "public"}])
        elif path == "/api/v2/instance":
            self.respond({"configuration": {"statuses": {"max_characters": 500}}})
        else:
            self.respond({"error": "NotFound"}, 404)

    def do_POST(self):
        path = urlparse(self.path).path
        REQUESTS.append(("POST", path, self.headers.get("Authorization")))
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        if path == "/xrpc/com.atproto.server.createSession" and body == {"identifier": HANDLE, "password": PASSWORD}:
            self.respond({"did": DID, "handle": HANDLE, "accessJwt": ACCESS, "refreshJwt": REFRESH,
                          "active": True, "didDoc": {"id": DID, "service": [{"id": "#atproto_pds",
                          "type": "AtprotoPersonalDataServer", "serviceEndpoint": self.server.origin}]}})
        else:
            self.respond({"error": "UnexpectedWrite"}, 400)


def hidden_login(binary, root, origin, env):
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(binary, [binary, "--config-dir", str(root), "login", "--provider", "bluesky",
                          HANDLE, "--pds", origin, "--label", "Blue smoke"], env)
    output = bytearray()
    sent = False
    deadline = time.monotonic() + 15
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([fd], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                output.extend(chunk)
                if not sent and b"App password (hidden):" in output:
                    # Waiting for raw mode protects against a prompt/echo race.
                    if not termios.tcgetattr(fd)[3] & termios.ECHO:
                        os.write(fd, PASSWORD.encode() + b"\r")
                        sent = True
            elif not sent and b"App password (hidden):" in output and not termios.tcgetattr(fd)[3] & termios.ECHO:
                os.write(fd, PASSWORD.encode() + b"\r")
                sent = True
        assert sent and b"Connected Blue smoke" in output, "Hidden app-password login failed"
        assert PASSWORD.encode() not in output and ACCESS.encode() not in output and REFRESH.encode() not in output
        assert termios.tcgetattr(fd)[3] & termios.ECHO
        assert termios.tcgetattr(fd)[3] & termios.ICANON
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0
    finally:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.close(fd)


def run():
    binary = str(Path(os.environ.get("TOOTLES_TEST_BINARY", "target/release/tootles")).resolve())
    with tempfile.TemporaryDirectory(prefix="tootles-bluesky-") as scratch:
        root = Path(scratch).resolve() / "private"
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.origin = f"http://127.0.0.1:{server.server_port}"
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        env = {**os.environ, "TERM": "xterm-256color", "COLORTERM": "truecolor"}
        env.pop("NO_COLOR", None)
        env.pop("TOOTLES_TOKEN", None)
        try:
            def cli(arguments, secret=None):
                result = subprocess.run([binary, "--config-dir", str(root), *arguments],
                                        input=secret, text=True, capture_output=True, env=env, timeout=15)
                assert result.returncode == 0, result.stderr
                assert PASSWORD not in result.stdout + result.stderr
                assert ACCESS not in result.stdout + result.stderr
                assert REFRESH not in result.stdout + result.stderr
                return result.stdout

            cli(["login", server.origin, "--token-stdin", "--label", "Masto smoke"], "mastodon-test-token")
            cli(["login", "--provider", "bluesky", HANDLE, "--pds", server.origin,
                 "--app-password-stdin", "--label", "Blue smoke"], PASSWORD)
            hidden_login(binary, root, server.origin, env)
            accounts = cli(["accounts"])
            assert "Masto smoke" in accounts and "Blue smoke" in accounts
            contents = (root / "config.toml").read_text()
            assert PASSWORD not in contents
            config = tomllib.loads(contents)
            assert len(config["accounts"]) == 2
            assert {account["provider"] for account in config["accounts"]} == {"mastodon", "bluesky"}

            pid, fd = pty.fork()
            if pid == 0:
                os.execve(binary, [binary, "--config-dir", str(root), "--no-motion", "--image-protocol", "halfblocks"], env)
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 144, 1296, 836))
            output = bytearray()

            def drain(seconds):
                until = time.monotonic() + seconds
                while time.monotonic() < until:
                    ready, _, _ = select.select([fd], [], [], max(0, until - time.monotonic()))
                    if ready:
                        try:
                            chunk = os.read(fd, 65536)
                        except OSError:
                            break
                        if not chunk:
                            break
                        output.extend(chunk)
                        if b"\x1b[6n" in chunk:
                            os.write(fd, b"\x1b[1;1R")
                        if b"\x1b[c" in chunk:
                            os.write(fd, b"\x1b[?1;2c")
                        if b"\x1b[16t" in chunk:
                            os.write(fd, b"\x1b[6;19;9t")
                        if b"\x1b[5n" in chunk:
                            os.write(fd, b"\x1b[0n")

            def send(value, wait=0.3):
                os.write(fd, value)
                drain(wait)

            def draft(text):
                send(b"n")
                send(("\x1b[200~" + text + "\x1b[201~").encode())
                send(b"\x1b")

            def switch(index):
                send(b"a")
                send(b"\x1b[H")
                for _ in range(index):
                    send(b"\x1b[B", 0.05)
                send(b"\r", 0.8)

            try:
                drain(1.5)
                assert b"Bluesky smoke marker" in output, "Bluesky timeline did not render: " + repr(bytes(output[-12000:]))
                draft("Only in my Bluesky draft")
                switch(1)
                assert b"Mastodon smoke marker" in output, "Mastodon account switch failed"
                draft("Only in my Mastodon draft")
                switch(2)
                output.clear()
                send(b"d")
                send(b"\r")
                assert b"Only in my Bluesky draft" in output, "Bluesky draft did not resume"
                assert b"Only in my Mastodon draft" not in output, "Draft crossed account boundary"
                send(b"\x1b")
                send(b"q", 0.5)
                for _ in range(10):
                    done, status = os.waitpid(pid, os.WNOHANG)
                    if done:
                        break
                    drain(0.1)
                else:
                    raise AssertionError("TUI did not quit")
                assert os.waitstatus_to_exitcode(status) == 0
                assert b"\x1b[?1049l" in output, "Terminal was not restored"
                assert PASSWORD.encode() not in output and ACCESS.encode() not in output
                assert REFRESH.encode() not in output
            finally:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                os.close(fd)
            assert all(auth == f"Bearer {ACCESS}" for method, path, auth in REQUESTS
                       if path.startswith("/xrpc/app.bsky."))
            assert not any(path.endswith("createRecord") for _, path, _ in REQUESTS)
            print("PASS: Bluesky CLI stdin/hidden login, token privacy, real timeline, Mastodon/Bluesky switching, separate drafts, and terminal restoration")
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    run()
