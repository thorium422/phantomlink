#!/bin/bash
# Run one qdisc through phantomlink's HTB shaper (the CLI feature added on
# `add-qdisc-support-clean`) under an iperf3 UDP overload. Intended for
# unattended CI use — outputs are written to $OUTDIR for later parsing by
# scripts/qdisc_ci_table.py.
#
# Differs from scripts/qdisc_udp_run.sh in that it uses the production
# --qdisc-client-shaper htb flag rather than hand-rewiring tc. So if a
# regression breaks the new shaper plumbing, this script catches it.
#
# Usage:
#   sudo ./scripts/qdisc_ci_run.sh <name> "<aqm tc tokens>" [options]
#
# Options:
#   --duration <s>     iperf3 -t test length (default 20).
#   --target <bw>      iperf3 -b target offered load (default 200M).
#   --outdir <dir>     output directory (default /tmp/qdisc_runs).

set -uo pipefail

if [ $# -lt 2 ]; then
    sed -n '2,/^$/p' "$0" >&2
    exit 1
fi

NAME="$1"; shift
QDISC="$1"; shift
DURATION=20
TARGET="200M"
OUTDIR="/tmp/qdisc_runs"
while [ $# -gt 0 ]; do
    case "$1" in
        --duration) DURATION="$2"; shift 2 ;;
        --target)   TARGET="$2"; shift 2 ;;
        --outdir)   OUTDIR="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

mkdir -p "$OUTDIR"
PHANTOM=./target/debug/phantomlink

echo "=== $NAME  htb-shaper UDP@$TARGET for ${DURATION}s — $QDISC ==="

$PHANTOM setup \
    --qdisc-client-shaper htb --qdisc-server-shaper htb \
    -c "$QDISC" -s "$QDISC" \
    > "$OUTDIR/setup_$NAME.log" 2>&1

$PHANTOM start examples/input.csv > "$OUTDIR/start_$NAME.log" 2>&1 &
START_PID=$!
sleep 1
$PHANTOM exec server iperf3 -s --port 5000 > "$OUTDIR/server_$NAME.txt" 2>&1 &
SERVER_PID=$!
sleep 1

# Poll the qdisc state every 0.5 s so the table generator can pick a
# mid-run sample.
N_SAMPLES=$(( (DURATION + 2) * 2 ))
( for i in $(seq 1 $N_SAMPLES); do
      awk -v i=$i 'BEGIN { printf "=== t=%.1f ===\n", i*0.5 }'
      ip netns exec pl_link tc -s qdisc show dev pqueue0_in
      sleep 0.5
  done
) > "$OUTDIR/qdisc_${NAME}_ts.txt" 2>&1 &
POLL_PID=$!

$PHANTOM exec client iperf3 -c 192.168.66.2 --port 5000 \
    -u -b "$TARGET" -t "$DURATION" --json \
    > "$OUTDIR/client_$NAME.json" 2>&1
CLIENT_STATUS=$?

kill "$POLL_PID" 2>/dev/null || true
kill "$SERVER_PID" 2>/dev/null || true
kill -INT "$START_PID" 2>/dev/null || true
sleep 2
$PHANTOM teardown > "$OUTDIR/teardown_$NAME.log" 2>&1 || true

if [ "$CLIENT_STATUS" -ne 0 ]; then
    echo "iperf3 client returned $CLIENT_STATUS for $NAME" >&2
fi

# One-line per-run summary so the CI log shows progress
python3 - "$OUTDIR/client_$NAME.json" <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as f:
        txt = f.read()
    i = txt.find('{')
    d = json.loads(txt[i:]) if i >= 0 else {}
    s = d.get('end', {}).get('sum', {}) or d.get('end', {}).get('sum_sent', {})
    print(f"  sender: {s.get('bits_per_second',0)/1e6:6.2f} Mbps  "
          f"pkts={s.get('packets','?')}  "
          f"lost={s.get('lost_packets','?')} ({s.get('lost_percent',0):.1f}%)")
except Exception as e:
    print(f"  (failed to parse iperf3 JSON: {e})")
PY
sleep 1
