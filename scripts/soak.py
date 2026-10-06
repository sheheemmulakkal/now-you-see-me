#!/usr/bin/env python3
"""Long-running soak check of an installed Now You See Me.

    scripts/soak.py run  OUT.csv [--hours 8] [--every 60] [--nysm PATH]
    scripts/soak.py report OUT.csv

`run` samples every running nysm process (service, tray, desktop, TUI) by
executable name: RSS, CPU time, open file descriptors and threads. It also
asks the service for its status once per sample (`nysm service status`, an IPC
round trip) and records whether it answered and how fast, and runs a local
`nysm summary --json` to count readings that are not "available". It never
starts or stops anything.

`report` prints per-process start/end RSS, RSS growth per hour (least squares),
mean CPU %, fd/thread ranges, service restarts (PID changes) and query failures.
"""

import csv
import json
import os
import subprocess
import sys
import time

TICK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
NAMES = ("nysm", "nysm-tray", "nysm-desktop")


def processes():
    out = []
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            exe = os.readlink(f"/proc/{pid}/exe")
            name = os.path.basename(exe).removesuffix(" (deleted)")
            if name not in NAMES:
                continue
            with open(f"/proc/{pid}/cmdline", "rb") as f:
                args = f.read().split(b"\0")
            role = name
            if name == "nysm":
                words = [a.decode(errors="replace") for a in args[1:3]]
                role = "nysm-" + "-".join(w for w in words if w and not w.startswith("-"))
            with open(f"/proc/{pid}/stat") as f:
                st = f.read().rsplit(")", 1)[1].split()
            with open(f"/proc/{pid}/statm") as f:
                rss = int(f.read().split()[1]) * PAGE
            fds = len(os.listdir(f"/proc/{pid}/fd"))
            out.append({
                "role": role,
                "pid": int(pid),
                "rss": rss,
                "cpu_ticks": int(st[11]) + int(st[12]),
                "threads": int(st[17]),
                "fds": fds,
            })
        except (OSError, ValueError, IndexError):
            continue  # exited or not ours
    return out


def count_unavailable(v):
    n = 0
    if isinstance(v, dict):
        if v.get("status") not in (None, "available") and "status" in v:
            n += 1
        for x in v.values():
            n += count_unavailable(x)
    elif isinstance(v, list):
        for x in v:
            n += count_unavailable(x)
    return n


def query(nysm):
    """(service answered, round trip ms, unavailable readings in a local summary)."""
    t = time.monotonic()
    try:
        ok = subprocess.run([nysm, "service", "status"], capture_output=True,
                            timeout=20).returncode == 0
    except (subprocess.TimeoutExpired, OSError):
        ok = False
    ms = (time.monotonic() - t) * 1000
    try:
        r = subprocess.run([nysm, "summary", "--json", "--top", "0"],
                           capture_output=True, timeout=30)
        unavail = count_unavailable(json.loads(r.stdout)) if r.returncode == 0 else -1
    except (subprocess.TimeoutExpired, json.JSONDecodeError, OSError):
        unavail = -1
    return ok, ms, unavail


def run(path, hours, every, nysm):
    end = time.time() + hours * 3600
    new = not os.path.exists(path)
    with open(path, "a", newline="") as f:
        w = csv.writer(f)
        if new:
            w.writerow(["time", "role", "pid", "rss", "cpu_ticks", "threads", "fds",
                        "query_ok", "query_ms", "unavailable"])
        while time.time() < end:
            ok, ms, unavail = query(nysm)
            now = int(time.time())
            for p in processes():
                w.writerow([now, p["role"], p["pid"], p["rss"], p["cpu_ticks"],
                            p["threads"], p["fds"], int(ok), f"{ms:.0f}", unavail])
            if not ok:
                w.writerow([now, "query", 0, 0, 0, 0, 0, 0, f"{ms:.0f}", unavail])
            f.flush()
            time.sleep(every)


def slope(xs, ys):
    n = len(xs)
    if n < 3:
        return None
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    return None if sxx == 0 else sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx


def report(path):
    rows = list(csv.DictReader(open(path)))
    if not rows:
        print("no samples")
        return
    t0, t1 = int(rows[0]["time"]), int(rows[-1]["time"])
    print(f"{path}: {(t1 - t0) / 3600:.1f} h, {len({r['time'] for r in rows})} samples")
    roles = sorted({r["role"] for r in rows} - {"query"})
    mib = 1024 * 1024
    for role in roles:
        rs = [r for r in rows if r["role"] == role]
        pids = []
        for r in rs:
            if not pids or pids[-1] != r["pid"]:
                pids.append(r["pid"])
        ts = [int(r["time"]) for r in rs]
        rss = [int(r["rss"]) for r in rs]
        # A growth rate needs a long window; startup ramp dominates short ones.
        g = slope([t / 3600 for t in ts], rss) if ts[-1] - ts[0] >= 1800 else None
        # CPU over the longest run of one PID.
        cpu = None
        last = [r for r in rs if r["pid"] == pids[-1]]
        if len(last) > 1:
            dt = int(last[-1]["time"]) - int(last[0]["time"])
            dc = int(last[-1]["cpu_ticks"]) - int(last[0]["cpu_ticks"])
            cpu = dc / TICK / dt * 100 if dt else None
        fds = [int(r["fds"]) for r in rs]
        th = [int(r["threads"]) for r in rs]
        print(f"  {role:<22} rss {rss[0] / mib:6.1f} → {rss[-1] / mib:6.1f} MiB"
              f"  growth {'—' if g is None else f'{g / mib:+.2f} MiB/h'}"
              f"  cpu {'—' if cpu is None else f'{cpu:.2f}%'}"
              f"  fds {min(fds)}–{max(fds)}  threads {min(th)}–{max(th)}"
              f"  pids {len(pids)}")
    q = {}
    for r in rows:
        q[r["time"]] = r
    qs = list(q.values())
    fails = sum(1 for r in qs if r["query_ok"] == "0")
    ms = sorted(float(r["query_ms"]) for r in qs if r["query_ok"] == "1")
    un = [int(r["unavailable"]) for r in qs]
    print(f"  service status checks: {len(qs)}, failed {fails}"
          + (f", round trip median {ms[len(ms) // 2]:.0f} ms, max {ms[-1]:.0f} ms" if ms else ""))
    print(f"  local summary: unavailable readings {min(un)}–{max(un)}"
          f" ({sum(1 for u in un if u < 0)} summaries failed)")


def main():
    a = sys.argv[1:]
    if len(a) >= 2 and a[0] == "report":
        return report(a[1])
    if len(a) >= 2 and a[0] == "run":
        opts = dict(zip(a[2::2], a[3::2]))
        return run(a[1], float(opts.get("--hours", 8)), float(opts.get("--every", 60)),
                   opts.get("--nysm", "nysm"))
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main()
