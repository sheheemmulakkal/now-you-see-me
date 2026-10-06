#!/usr/bin/env python3
"""Drive `nysm tui` in a real pseudo-terminal and check terminal hygiene.

Checks: alternate screen entered and left, cursor restored, clean exit on
`q`, and that the process exits (rather than spinning) when the terminal
goes away (SSH disconnect simulation). Usage: scripts/pty_smoke.py [binary]
"""
import fcntl, os, pty, select, signal, struct, sys, termios, time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/nysm"

def spawn(cols=80, rows=24, extra=()):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execv(BIN, [BIN, "tui", *extra])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    return pid, fd

def drain(fd, secs):
    out = b""
    end = time.time() + secs
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
    return out

def wait_exit(pid, secs):
    end = time.time() + secs
    while time.time() < end:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return os.waitstatus_to_exitcode(status)
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    return None

failures = []
def check(cond, msg):
    print(("ok   " if cond else "FAIL ") + msg)
    if not cond:
        failures.append(msg)

# 1. Normal session: render, resize, navigate, quit.
pid, fd = spawn()
out = drain(fd, 2.5)
check(b"\x1b[?1049h" in out, "enters alternate screen")
check(b"Overview" in out and b"CPU" in out, "renders overview")
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
os.kill(pid, signal.SIGWINCH)
for key in [b"2", b"j", b"j", b"t", b"/", b"sh", b"\r", b" ", b"?", b"x", b"3", b"6"]:
    os.write(fd, key)
    out += drain(fd, 0.15)
check(b"PAUSED" in out, "pause banner shown")
os.write(fd, b"q")
out += drain(fd, 1.0)
code = wait_exit(pid, 3)
check(code == 0, f"exits 0 on q (got {code})")
check(out.rfind(b"\x1b[?1049l") > out.rfind(b"\x1b[?1049h"), "leaves alternate screen last")
check(b"\x1b[?25h" in out[out.rfind(b"\x1b[?1049h"):], "cursor shown again")

# 2. Terminal disappears (SSH disconnect): process must exit, not spin.
pid, fd = spawn()
drain(fd, 1.5)
os.close(fd)
code = wait_exit(pid, 5)
check(code is not None, f"exits after terminal hangup (code {code})")

# 3. Tiny terminal.
pid, fd = spawn(cols=30, rows=8)
out = drain(fd, 2.0)
os.write(fd, b"q")
code = wait_exit(pid, 3)
check(b"enlarge" in out and code == 0, "tiny terminal summary + clean exit")

sys.exit(1 if failures else 0)
