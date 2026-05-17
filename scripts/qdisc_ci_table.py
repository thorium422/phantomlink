#!/usr/bin/env python3
"""Emit a Markdown table from a /tmp/qdisc_runs/ directory produced by
scripts/qdisc_ci_sweep.sh. Designed to be appended to $GITHUB_STEP_SUMMARY.

For each `client_<name>.json` we join iperf3's sender/lost stats with the
mid-run (t~15s) sample from `qdisc_<name>_ts.txt`. The leaf qdisc in that
sample is the AQM under HTB, so its `backlog`, `dropped`, and AQM-specific
counters tell us whether the AQM engaged the way it should.

Usage: qdisc_ci_table.py [outdir]   (default /tmp/qdisc_runs)
"""
import json
import re
import sys
from pathlib import Path

D = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/qdisc_runs")
clients = sorted(D.glob("client_*.json"))

print("## Qdisc UDP-overload benchmark")
print()
print("Each AQM is wrapped in HTB at the scenario's bottleneck rate via the")
print("`--qdisc-*-shaper htb` CLI flag — see")
print("[`docs/qdisc-design-rationale.md`](../blob/HEAD/docs/qdisc-design-rationale.md)")
print("for why classless AQMs need a co-located rate limit. iperf3 offers")
print("200 Mbps UDP into a scenario whose bottleneck is 80–100 Mbps, so the AQM")
print("is expected to drop the excess.")
print()
print("| qdisc | sender (Mbps) | lost % | pkts | queue @ t≈15s | AQM drops | leaf detail |")
print("|---|---:|---:|---:|---:|---:|---|")

if not clients:
    print("| _no client_\\*.json found in output dir_ | | | | | | |")
    sys.exit(0)


def parse_leaf_sample(ts_path: Path, target_t: float = 15.0):
    """Return (leaf_block_text, sample_t) for the qdisc sample closest to
    `target_t`. None if no samples are available."""
    if not ts_path.exists():
        return None, None
    text = ts_path.read_text()
    parts = re.split(r"^=== t=([\d.]+) ===\s*$", text, flags=re.M)[1:]
    best = None
    best_dt = None
    for j in range(0, len(parts), 2):
        try:
            t = float(parts[j])
        except ValueError:
            continue
        dt = abs(t - target_t)
        if best is None or dt < best_dt:
            best_dt = dt
            best = (t, parts[j + 1])
    if best is None:
        return None, None
    sample_t, body = best
    # The leaf qdisc is the *last* `qdisc ...` block in the sample (root htb
    # comes first; the AQM child comes second).
    blocks = body.split("qdisc ")
    leaf = "qdisc " + (blocks[-1] if len(blocks) > 1 else "")
    return leaf, sample_t


for c in clients:
    name = c.stem.replace("client_", "")

    try:
        txt = c.read_text()
        i = txt.find("{")
        d = json.loads(txt[i:]) if i >= 0 else {}
    except Exception:
        d = {}
    s = d.get("end", {}).get("sum", {}) or d.get("end", {}).get("sum_sent", {})
    mbps = s.get("bits_per_second", 0) / 1e6
    lost_pct = s.get("lost_percent", 0)
    pkts = s.get("packets", 0)

    leaf, _ = parse_leaf_sample(D / f"qdisc_{name}_ts.txt")

    queue_cell = "—"
    drops_cell = "—"
    detail_cell = "—"
    if leaf:
        mq = re.search(r"backlog \d+b (\d+)p", leaf)
        if mq:
            queue_cell = f"{int(mq.group(1)):,}p"
        md = re.search(r"dropped (\d+),", leaf)
        if md:
            drops_cell = f"{int(md.group(1)):,}"

        parts = []
        for label, pat in (
            ("ldelay", r"ldelay (\S+)"),
            ("ecn_mark", r"ecn_mark (\d+)"),
            ("early", r"early (\d+)"),
            ("pdrop", r"pdrop (\d+)"),
            ("drop_ovl", r"drop_overlimit (\d+)"),
        ):
            m = re.search(pat, leaf)
            if m and m.group(1) not in ("0",):
                parts.append(f"`{label}={m.group(1)}`")
        if "dropping" in leaf:
            parts.append("`DROPPING`")
        if parts:
            detail_cell = " ".join(parts)

    print(
        f"| `{name}` | {mbps:.1f} | {lost_pct:.1f}% | {pkts:,} | "
        f"{queue_cell} | {drops_cell} | {detail_cell} |"
    )
