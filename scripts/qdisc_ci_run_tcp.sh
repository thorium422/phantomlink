#!/bin/bash
# Run one (AQM × TCP congestion-control algorithm) pair through phantomlink
# with the HTB shaper, mirroring the paper's §4 experiment but cross-cutting
# every supported AQM. Sibling script to qdisc_ci_run.sh (which does UDP
# overload); see docs/qdisc-design-rationale.md for why we need the HTB
# wrapper.
#
# Usage:
#   sudo ./scripts/qdisc_ci_run_tcp.sh <qdisc-name> "<aqm tokens>" <cca> [options]
#
# Options:
#   --duration <s>     iperf3 -t test length (default 25, just covers the
#                       100→80 Mbps route change at t=22s in examples/input.csv).
#   --outdir <dir>     output dir (default /tmp/qdisc_runs).
#
# Output (in $OUTDIR):
#   client_tcp_<qdisc>_<cca>.json      iperf3 --json client output
#   server_tcp_<qdisc>_<cca>.txt
#   qdisc_tcp_<qdisc>_<cca>_ts.txt     tc snapshots prefixed by t=
#   setup_tcp_<qdisc>_<cca>.log
#   start_tcp_<qdisc>_<cca>.log
#   teardown_tcp_<qdisc>_<cca>.log

set -uo pipefail

if [ $# -lt 3 ]; then
    sed -n '2,/^$/p' "$0" >&2
    exit 1
fi

QDISC_NAME="$1"; shift
QDISC="$1"; shift
CCA="$1"; shift
DURATION=25
OUTDIR="/tmp/qdisc_runs"
while [ $# -gt 0 ]; do
    case "$1" in
        --duration) DURATION="$2"; shift 2 ;;
        --outdir)   OUTDIR="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

NAME="${QDISC_NAME}_${CCA}"
mkdir -p "$OUTDIR"
PHANTOM=./target/debug/phantomlink

echo "=== tcp_${NAME}  htb-shaper TCP -C $CCA for ${DURATION}s — $QDISC ==="

$PHANTOM setup \
    --qdisc-client-shaper htb --qdisc-server-shaper htb \
    -c "$QDISC" -s "$QDISC" \
    > "$OUTDIR/setup_tcp_$NAME.log" 2>&1

$PHANTOM start examples/input.csv > "$OUTDIR/start_tcp_$NAME.log" 2>&1 &
START_PID=$!
sleep 1
$PHANTOM exec server iperf3 -s --port 5000 > "$OUTDIR/server_tcp_$NAME.txt" 2>&1 &
SERVER_PID=$!
sleep 1

N_SAMPLES=$(( (DURATION + 2) * 2 ))
( for i in $(seq 1 $N_SAMPLES); do
      awk -v i=$i 'BEGIN { printf "=== t=%.1f ===\n", i*0.5 }'
      ip netns exec pl_link tc -s qdisc show dev pqueue0_in
      sleep 0.5
  done
) > "$OUTDIR/qdisc_tcp_${NAME}_ts.txt" 2>&1 &
POLL_PID=$!

$PHANTOM exec client iperf3 -c 192.168.66.2 --port 5000 \
    -C "$CCA" -t "$DURATION" --json \
    > "$OUTDIR/client_tcp_$NAME.json" 2>&1
CLIENT_STATUS=$?

kill "$POLL_PID" 2>/dev/null || true
kill "$SERVER_PID" 2>/dev/null || true
kill -INT "$START_PID" 2>/dev/null || true
sleep 2
$PHANTOM teardown > "$OUTDIR/teardown_tcp_$NAME.log" 2>&1 || true

if [ "$CLIENT_STATUS" -ne 0 ]; then
    echo "iperf3 TCP client returned $CLIENT_STATUS for $NAME" >&2
fi

# One-line summary so CI log shows progress
python3 - "$OUTDIR/client_tcp_$NAME.json" "$CCA" <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as f:
        txt = f.read()
    i = txt.find('{')
    d = json.loads(txt[i:]) if i >= 0 else {}
    e = d.get('end', {})
    s = e.get('sum_sent', {})
    r = e.get('sum_received', {})
    streams = e.get('streams', [])
    sender = streams[0].get('sender', {}) if streams else {}
    rtx = sender.get('retransmits', '?')
    mean_rtt_us = sender.get('mean_rtt', 0) or 0
    print(f"  cca={sys.argv[2]}  send={s.get('bits_per_second',0)/1e6:6.2f} Mbps  "
          f"recv={r.get('bits_per_second',0)/1e6:6.2f} Mbps  "
          f"retransmits={rtx}  mean_rtt={mean_rtt_us/1000:.1f}ms")
except Exception as exc:
    print(f"  (failed to parse iperf3 JSON: {exc})")
PY
sleep 1
