#!/bin/bash
# Run a single qdisc against an iperf3 UDP overload, polling `tc -s qdisc show`
# every 0.5s for the duration of the test.
#
# Usage:
#   sudo ./scripts/qdisc_udp_run.sh <name> "<qdisc tokens>" [options]
#
# Options:
#   --rate <mbit>      HTB ceiling rate (default 80 — at the lower end of
#                      the bandwidths in examples/input.csv).
#   --target <bw>      iperf3 -b target offered load (default 200M).
#   --duration <s>     iperf3 -t test length (default 30).
#   --no-htb           attach <qdisc tokens> as root, no HTB wrapper.
#   --outdir <dir>     output directory (default /tmp/qdisc_runs).
#
# Output files in <outdir>:
#   client_<name>.json      iperf3 --json client output
#   server_<name>.txt       iperf3 server output
#   qdisc_<name>_ts.txt     tc snapshots prefixed with "=== t=<sec> ==="
#   setup_<name>.log        phantomlink setup output
#   start_<name>.log        phantomlink start output
#   teardown_<name>.log     phantomlink teardown output
#   summary_<name>.txt      one-line iperf3 result

set -uo pipefail

RATE=80
TARGET="200M"
DURATION=30
HTB=1
OUTDIR="/tmp/qdisc_runs"

if [ $# -lt 2 ]; then
    sed -n '2,/^$/p' "$0" >&2
    exit 1
fi
NAME="$1"; shift
QDISC="$1"; shift
while [ $# -gt 0 ]; do
    case "$1" in
        --rate)     RATE="$2"; shift 2 ;;
        --target)   TARGET="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --no-htb)   HTB=0; shift ;;
        --outdir)   OUTDIR="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

mkdir -p "$OUTDIR"
PHANTOM=./target/debug/phantomlink

if [ "$HTB" = "1" ]; then
    echo "=== $NAME  htb@${RATE}mbit ─ UDP@$TARGET for ${DURATION}s ─ $QDISC ==="
    # Placeholder qdisc on setup; we override pqueue0_in below.
    sudo $PHANTOM setup --qdisc-client "pfifo limit 1" --qdisc-server "pfifo limit 1" \
        > "$OUTDIR/setup_$NAME.log" 2>&1
    sudo ip netns exec pl_link tc qdisc del dev pqueue0_in root 2>/dev/null || true
    sudo ip netns exec pl_link tc qdisc add dev pqueue0_in root handle 1: htb default 10
    sudo ip netns exec pl_link tc class add dev pqueue0_in parent 1: classid 1:10 \
        htb rate "${RATE}mbit" ceil "${RATE}mbit"
    # shellcheck disable=SC2086
    sudo ip netns exec pl_link tc qdisc add dev pqueue0_in parent 1:10 handle 10: $QDISC
else
    echo "=== $NAME  root (no htb) ─ UDP@$TARGET for ${DURATION}s ─ $QDISC ==="
    sudo $PHANTOM setup --qdisc-client "$QDISC" --qdisc-server "$QDISC" \
        > "$OUTDIR/setup_$NAME.log" 2>&1
fi
echo "  qdisc tree:"
sudo ip netns exec pl_link tc qdisc show dev pqueue0_in | sed 's/^/    /'

sudo $PHANTOM start examples/input.csv > "$OUTDIR/start_$NAME.log" 2>&1 &
START_PID=$!
sleep 1
sudo $PHANTOM exec server iperf3 -s --port 5000 > "$OUTDIR/server_$NAME.txt" 2>&1 &
SERVER_PID=$!
sleep 1

# poll every 0.5s for DURATION+2 seconds
N_SAMPLES=$(( (DURATION + 2) * 2 ))
( for i in $(seq 1 $N_SAMPLES); do
      awk -v i=$i 'BEGIN { printf "=== t=%.1f ===\n", i*0.5 }'
      sudo ip netns exec pl_link tc -s qdisc show dev pqueue0_in
      sleep 0.5
  done
) > "$OUTDIR/qdisc_${NAME}_ts.txt" 2>&1 &
POLL_PID=$!

sudo $PHANTOM exec client iperf3 -c 192.168.66.2 --port 5000 \
    -u -b "$TARGET" -t "$DURATION" --json \
    > "$OUTDIR/client_$NAME.json" 2>&1 &
CLIENT_PID=$!
wait $CLIENT_PID

sudo kill $POLL_PID 2>/dev/null
sudo kill $SERVER_PID 2>/dev/null
sudo kill -INT $START_PID 2>/dev/null
sleep 2
sudo $PHANTOM teardown > "$OUTDIR/teardown_$NAME.log" 2>&1

python3 - "$OUTDIR/client_$NAME.json" "$OUTDIR/summary_$NAME.txt" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    txt = f.read()
i = txt.find('{')
d = json.loads(txt[i:]) if i >= 0 else {}
s = d.get('end', {}).get('sum', {}) or d.get('end', {}).get('sum_sent', {})
out = (f"sender: {s.get('bits_per_second',0)/1e6:6.2f} Mbps  "
       f"packets={s.get('packets','?')}  "
       f"lost={s.get('lost_packets','?')} ({s.get('lost_percent',0):.1f}%)  "
       f"duration={s.get('seconds',0):.1f}s")
print("  " + out)
with open(sys.argv[2], 'w') as f:
    f.write(out + "\n")
PY
sleep 1
