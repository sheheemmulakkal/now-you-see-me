#!/usr/bin/env python3
"""List third-party crates linked into the shipped binaries with their licences."""
import json, subprocess

meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked"], text=True))
ws = set(meta["workspace_members"])
pkgs = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
roots = [p["id"] for p in meta["packages"] if p["name"] in ("nysm-cli", "nysm-tray", "nysm-desktop") and p["id"] in ws]
seen, stack = set(), list(roots)
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        # normal (non-dev, non-build) dependencies only
        if any(k.get("kind") is None for k in d["dep_kinds"]):
            stack.append(d["pkg"])
print("Third-party components in Now You See Me binaries\n")
print("Now You See Me is licensed under MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).\n")
for i in sorted(seen - ws, key=lambda i: pkgs[i]["name"]):
    p = pkgs[i]
    print(f'{p["name"]} {p["version"]}: {p.get("license") or p.get("license_file") or "UNKNOWN"}')
