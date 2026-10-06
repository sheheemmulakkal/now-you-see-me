#!/usr/bin/env python3
"""Measure nysm overhead externally via /proc/<pid>/{stat,status}.

Runs scenarios concurrently for DURATION seconds and reports average CPU
(% of one logical core), peak RSS (VmHWM) and final RSS, plus voluntary
context switches/s as a proxy for wakeups. Usage:
  scripts/measure.py [binary] [seconds]
"""
import os, pty, sys, time, subprocess, fcntl, struct, termios, select

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/nysm"
DURATION = float(sys.argv[2]) if len(sys.argv) > 2 else 60
TCK = os.sysconf("SC_CLK_TCK")

def cpu_ticks(pid):
    with open(f"/proc/{pid}/stat") as f:
        rest = f.read().rsplit(")", 1)[1].split()
    return int(rest[11]) + int(rest[12])

def status(pid):
    d = {}
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            k, _, v = line.partition(":")
            d[k] = v.strip()
    return d

def kb(s):
    return int(s.split()[0])

scen = {}
devnull = open(os.devnull, "w")
scen["watch 1s (no processes)"] = subprocess.Popen([BIN, "watch", "--interval", "1s", "--format", "jsonl"], stdout=devnull)
scen["watch 1s + processes"] = subprocess.Popen([BIN, "watch", "--interval", "1s", "--format", "jsonl", "--processes"], stdout=devnull)
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execv(BIN, [BIN, "tui"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
class P:
    def __init__(s, pid): s.pid = pid
scen["tui 80x24 idle (processes on)"] = P(pid)

time.sleep(3)  # skip startup
start = {k: (cpu_ticks(p.pid), kb(status(p.pid)["voluntary_ctxt_switches"] + " x"), time.time()) for k, p in scen.items()}
end_t = time.time() + DURATION
while time.time() < end_t:
    r, _, _ = select.select([fd], [], [], 0.5)
    if r:
        os.read(fd, 65536)  # keep the pty drained like a real terminal
res = {}
for k, p in scen.items():
    t0, cs0, w0 = start[k]
    st = status(p.pid)
    el = time.time() - w0
    res[k] = (
        (cpu_ticks(p.pid) - t0) / TCK / el * 100,
        kb(st["VmHWM"]) / 1024,
        kb(st["VmRSS"]) / 1024,
        (kb(st["voluntary_ctxt_switches"] + " x") - cs0) / el,
        int(st["Threads"]),
    )
os.write(fd, b"q")
for k, p in scen.items():
    if hasattr(p, "terminate"):
        p.terminate(); p.wait()
os.waitpid(pid, 0)
print(f"duration {DURATION:.0f}s, CLK_TCK {TCK}, {os.cpu_count()} logical CPUs, procs visible ~{len([d for d in os.listdir('/proc') if d.isdigit()])}")
print(f"{'scenario':32} {'cpu %1core':>10} {'peak RSS':>9} {'RSS':>8} {'ctxsw/s':>8} {'thr':>4}")
for k, (c, hwm, rss, cs, thr) in res.items():
    print(f"{k:32} {c:10.2f} {hwm:7.1f}Mi {rss:6.1f}Mi {cs:8.1f} {thr:4}")
