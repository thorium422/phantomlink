#!/usr/bin/env python3
"""Walk an output dir produced by qdisc_udp_run.sh and print a result table.
Usage: qdisc_udp_summarize.py [outdir]   (default /tmp/qdisc_runs)
"""
import json
import re
import sys
from pathlib import Path

D = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/qdisc_runs")
clients = sorted(D.glob("client_*.json"))
if not clients:
    print(f"no client_*.json in {D}", file=sys.stderr)
    sys.exit(1)

header = f"{'qdisc':<10} {'lost%':>7} {'pkts':>8} {'queue@t15':>10} {'ldelay':>8} {'drops':>9}  detail"
print(header)
print("-" * len(header))

for c in clients:
    name = c.stem.replace("client_", "")
    ts = D / f"qdisc_{name}_ts.txt"
    if not ts.exists():
        continue
    txt = c.read_text()
    i = txt.find("{")
    d = json.loads(txt[i:]) if i >= 0 else {}
    s = d.get("end", {}).get("sum", {}) or d.get("end", {}).get("sum_sent", {})
    lost = s.get("lost_percent", 0)
    pkts = s.get("packets", 0)

    text = ts.read_text()
    samples = re.split(r"^=== t=([\d.]+) ===\s*$", text, flags=re.M)[1:]
    chosen = None
    for j in range(0, len(samples), 2):
        if abs(float(samples[j]) - 15.0) < 0.3:
            chosen = samples[j + 1]
            break
    if not chosen:
        print(f"{name:<10} (no mid-run sample at t≈15s)")
        continue

    # Take the *last* qdisc block (the leaf — AQM under HTB if nested,
    # else the root itself).
    blocks = chosen.split("qdisc ")
    leaf = "qdisc " + (blocks[-1] if len(blocks) > 1 else "")
    drops = re.search(r"dropped (\d+),", leaf)
    blq_p = re.search(r"backlog \d+b (\d+)p", leaf)
    ldelay = re.search(r"ldelay (\S+)", leaf)
    detail = []
    for label, pat in (("ecn_mark", r"ecn_mark (\d+)"),
                       ("early",    r"early (\d+)"),
                       ("pdrop",    r"pdrop (\d+)"),
                       ("drop_ovl", r"drop_overlimit (\d+)")):
        m = re.search(pat, leaf)
        if m and m.group(1) != "0":
            detail.append(f"{label}={m.group(1)}")
    if "dropping" in leaf:
        detail.append("DROPPING")

    print(f"{name:<10} {lost:>6.1f}% {pkts:>8} "
          f"{(blq_p.group(1) + 'p') if blq_p else '-':>10} "
          f"{(ldelay.group(1) if ldelay else '-'):>8} "
          f"{(drops.group(1) if drops else '-'):>9}  "
          f"{' '.join(detail)}")
