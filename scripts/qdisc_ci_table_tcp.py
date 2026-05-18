#!/usr/bin/env python3
"""Emit a Markdown table for the (AQM + CCA) TCP sweep produced by
scripts/qdisc_ci_sweep_tcp.sh.
Usage: qdisc_ci_table_tcp.py [outdir]   (default /tmp/qdisc_runs)
"""
import json
import re
import sys
from pathlib import Path

D = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/qdisc_runs")
clients = sorted(D.glob("client_tcp_*.json"))

# Order CCAs in the table the way the paper compares them.
CCA_ORDER = ("reno", "cubic", "bbr", "bbr2", "vegas", "htcp")

print("## Congestion-controller + AQM sweep (TCP)")
print("")
print("| qdisc | CCA | sender Mbps | receiver Mbps | retransmits | mean RTT (ms) | min/max RTT (ms) |")
print("|---|---|---:|---:|---:|---:|---:|")

if not clients:
    print("| _no client_tcp_\\*.json found in output dir_ | | | | | | |")
    sys.exit(0)


def split_name(stem: str):
    """`<qdisc>_<cca>` where qdisc may itself contain underscores
    (`fq_codel`, `pfifo_head_drop`)."""
    name = stem.replace("client_tcp_", "")
    for cca in CCA_ORDER:
        if name.endswith("_" + cca):
            return name[: -(len(cca) + 1)], cca
    parts = name.rsplit("_", 1)
    return (parts[0], parts[1]) if len(parts) == 2 else (name, "?")


def sort_key(c: Path):
    qd, cca = split_name(c.stem)
    cca_idx = CCA_ORDER.index(cca) if cca in CCA_ORDER else 999
    return (qd, cca_idx)


for c in sorted(clients, key=sort_key):
    qdisc, cca = split_name(c.stem)

    try:
        txt = c.read_text()
        i = txt.find("{")
        d = json.loads(txt[i:]) if i >= 0 else {}
    except Exception:
        d = {}
    e = d.get("end", {})
    s = e.get("sum_sent", {})
    r = e.get("sum_received", {})
    streams = e.get("streams", [])
    sender = streams[0].get("sender", {}) if streams else {}

    send_mbps = (s.get("bits_per_second") or 0) / 1e6
    recv_mbps = (r.get("bits_per_second") or 0) / 1e6
    rtx = sender.get("retransmits", 0) or 0
    mean_rtt_ms = (sender.get("mean_rtt") or 0) / 1000.0
    min_rtt_ms = (sender.get("min_rtt") or 0) / 1000.0
    max_rtt_ms = (sender.get("max_rtt") or 0) / 1000.0

    print(
        f"| `{qdisc}` | `{cca}` | "
        f"{send_mbps:.1f} | {recv_mbps:.1f} | "
        f"{rtx:,} | "
        f"{mean_rtt_ms:.1f} | "
        f"{min_rtt_ms:.1f} / {max_rtt_ms:.1f} |"
    )
