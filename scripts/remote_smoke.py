#!/usr/bin/env python3
"""End-to-end test of `nysm tui --remote` without an SSH server.

A stand-in `ssh` placed first in PATH drops the SSH options and runs the
remote command locally, so the TUI -> transport -> `nysm service stdio`
path is exercised exactly as with real SSH, minus SSH itself.

Usage: scripts/remote_smoke.py [path/to/nysm]
"""
import fcntl, os, pty, select, signal, struct, sys, tempfile, termios, time

B = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/nysm")
tmp = tempfile.mkdtemp(prefix="nysm-fakessh-")
log = os.path.join(tmp, "calls.log")
with open(os.path.join(tmp, "ssh"), "w") as f:
    f.write(f"""#!/bin/sh
echo "$*" >> {log}
while [ "$1" != "--" ]; do shift; done
shift; shift
exec "$@"
""")
os.chmod(os.path.join(tmp, "ssh"), 0o755)

failures = []
def check(ok, msg):
    print(("ok   " if ok else "FAIL ") + msg)
    if not ok:
        failures.append(msg)

def stdio_servers():
    pids = []
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                argv = open(f"/proc/{d}/cmdline", "rb").read().split(b"\0")
            except OSError:
                continue
            if argv[0] == B.encode() and b"stdio" in argv:
                pids.append(int(d))
    return pids

pid, fd = pty.fork()
if pid == 0:
    os.environ["PATH"] = tmp + ":" + os.environ["PATH"]
    os.environ["TERM"] = "xterm"
    os.execv(B, [B, "tui", "--remote", "me@example", "--remote-nysm", B])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))

def drain(t):
    out = b""
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    return out

out = drain(3.0)
check(b"Overview" in out and b"CPU" in out, "remote TUI renders data")
check(open(log).read().split()[:2] == ["-T", "--"], "ssh invoked with -T -- DEST")
servers = stdio_servers()
check(len(servers) == 1, f"one remote stdio server running ({len(servers)})")
for p in servers:
    os.kill(p, signal.SIGTERM)
out = drain(2.0)
check(b"remote connection lost" in out, "connection loss is shown")
check(b"collecting locally" not in out, "no silent fallback to local data")
os.write(fd, b"q")
drain(0.5)
_, st = os.waitpid(pid, 0)
check(os.waitstatus_to_exitcode(st) == 0, "TUI exits cleanly")
time.sleep(0.3)
check(not stdio_servers(), "no stdio server left behind")
# Second session: quitting normally must also end the remote side.
pid, fd = pty.fork()
if pid == 0:
    os.environ["PATH"] = tmp + ":" + os.environ["PATH"]
    os.environ["TERM"] = "xterm"
    os.execv(B, [B, "tui", "--remote", "me@example", "--remote-nysm", B])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
drain(2.0)
os.write(fd, b"q")
drain(0.5)
os.waitpid(pid, 0)
time.sleep(0.5)
check(not stdio_servers(), "quitting the TUI ends the remote collector")
sys.exit(1 if failures else 0)
