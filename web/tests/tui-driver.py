#!/usr/bin/env python3
"""Run the real xo TUI in a PTY and expose admission approval for Playwright."""

import argparse
import errno
import fcntl
import http.server
import json
import os
import pty
import select
import signal
import struct
import termios
import threading
import time


parser = argparse.ArgumentParser()
parser.add_argument("--bind", default="127.0.0.1")
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--log", required=True)
parser.add_argument("--ready-text", default="Browser fixture")
parser.add_argument("command", nargs=argparse.REMAINDER)
args = parser.parse_args()
if args.command and args.command[0] == "--":
    args.command = args.command[1:]
if not args.command:
    parser.error("a TUI command is required after --")
ready_text = args.ready_text.encode()

approval_requested = threading.Event()
approval_finished = threading.Event()
ready = threading.Event()
stopping = threading.Event()


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/healthz":
            self.send_error(404)
            return
        self.send_response(200 if ready.is_set() else 503)
        self.end_headers()

    def do_POST(self):
        if self.path != "/approve":
            self.send_error(404)
            return
        approval_requested.set()
        completed = approval_finished.wait(120)
        body = json.dumps({"approved": completed}).encode()
        self.send_response(200 if completed else 504)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


server = http.server.ThreadingHTTPServer((args.bind, args.port), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()

pid, terminal = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp(args.command[0], args.command)

fcntl.ioctl(terminal, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 180, 0, 0))
fcntl.fcntl(terminal, fcntl.F_SETFL, fcntl.fcntl(terminal, fcntl.F_GETFL) | os.O_NONBLOCK)


def stop(*_args):
    stopping.set()


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
opened_devices = False
selected_notes_view = False
last_approval_key = 0.0
transcript = bytearray()
status = 1

try:
    with open(args.log, "ab", buffering=0) as log:
        while not stopping.is_set():
            waited, child_status = os.waitpid(pid, os.WNOHANG)
            if waited:
                status = os.waitstatus_to_exitcode(child_status)
                break
            readable, _, _ = select.select([terminal], [], [], 0.1)
            if readable:
                try:
                    data = os.read(terminal, 65536)
                except BlockingIOError:
                    data = b""
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                if data:
                    log.write(data)
                    transcript.extend(data)
                    if len(transcript) > 1_000_000:
                        del transcript[:-500_000]
                    if ready_text in transcript:
                        ready.set()
                    if b"approved peer" in transcript:
                        approval_finished.set()
            if b"[Space] menu" in transcript and not ready.is_set() and not selected_notes_view:
                # The CI fixture defaults to Library. Exercise the real `g` view
                # switcher and select the unique Notes match before declaring ready.
                os.write(terminal, b"g")
                time.sleep(0.2)
                os.write(terminal, b"n")
                selected_notes_view = True
            if approval_requested.is_set() and not opened_devices:
                os.write(terminal, b" ")
                time.sleep(0.2)
                os.write(terminal, b"i")
                opened_devices = True
            if opened_devices and not approval_finished.is_set() and time.monotonic() - last_approval_key > 1:
                os.write(terminal, b"a")
                last_approval_key = time.monotonic()
finally:
    server.shutdown()
    if not approval_finished.is_set():
        approval_finished.set()
    try:
        os.write(terminal, b"\x1b")
        time.sleep(0.2)
        os.write(terminal, b"q")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            waited, _ = os.waitpid(pid, os.WNOHANG)
            if waited:
                pid = 0
                break
            time.sleep(0.1)
        if pid:
            os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pid = 0
    if pid:
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
    os.close(terminal)

raise SystemExit(status if not stopping.is_set() else 0)
