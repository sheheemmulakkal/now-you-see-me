#!/usr/bin/env python3
"""Compare 2 TUIs collecting locally vs 2 TUIs attached to one service.

Usage: scripts/measure_service.py [binary] [seconds]
Uses NYSM_RUNTIME_DIR=/tmp/nysm-measure-<pid> (short path for the socket).
"""
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/nysm"
DUR = float(sys.argv[2]) if len(sys.argv) > 2 else 30
TCK = os.sysconf("SC_CLK_TCK")
RT = f"/tmp/nysm-measure-{os.getpid()}"
os.environ["NYSM_RUNTIME_DIR"] = RT

def ticks(pid):
    with open(f"/proc/{pid}/stat") as f:
        r = f.read().rsplit(")", 1)[1].split()
    return int(r[11]) + int(r[12])

def rss_mib(pid):
    with open(f"/proc/{pid}/status") as f:
        for l in f:
            if l.startswith("VmRSS"):
                return int(l.split()[1]) / 1024

def tui(attach):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execv(BIN, [BIN, "tui", "--attach", attach])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    return pid, fd

def measure(procs, fds):
    time.sleep(3)
    t0 = {n: ticks(p) for n, p in procs.items()}
    w0 = time.time()
    end = w0 + DUR
    while time.time() < end:
        r, _, _ = select.select(fds, [], [], 0.5)
        for fd in r:
            try: os.read(fd, 65536)
            except OSError: pass
    el = time.time() - w0
    return {n: ((ticks(p) - t0[n]) / TCK / el * 100, rss_mib(p)) for n, p in procs.items()}

def stop(pids, fds):
    for fd in fds:
        try: os.write(fd, b"q")
        except OSError: pass
    for p in pids:
        try: os.waitpid(p, 0)
        except ChildProcessError: pass

# A: two standalone TUIs
a1, f1 = tui("never"); a2, f2 = tui("never")
ra = measure({"tui#1 local": a1, "tui#2 local": a2}, [f1, f2])
stop([a1, a2], [f1, f2])

# B: service + two attached TUIs
svc = subprocess.Popen([BIN, "service", "run"], stderr=subprocess.DEVNULL)
time.sleep(1)
b1, g1 = tui("require"); b2, g2 = tui("require")
rb = measure({"service": svc.pid, "tui#1 attached": b1, "tui#2 attached": b2}, [g1, g2])
stop([b1, b2], [g1, g2])
svc.terminate(); svc.wait()
shutil.rmtree(RT, ignore_errors=True)

print(f"{DUR:.0f}s each; CPU % of one core, RSS MiB")
for title, r in (("A: two local TUIs", ra), ("B: service + two attached TUIs", rb)):
    total = sum(c for c, _ in r.values())
    print(f"{title}  (total CPU {total:.2f}%)")
    for n, (c, m) in r.items():
        print(f"  {n:18} {c:6.2f}%  {m:5.1f} MiB")
