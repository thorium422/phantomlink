#!/bin/bash
# Drive scripts/qdisc_ci_run_tcp.sh across every supported (AQM × CCA) pair.
# Companion to qdisc_ci_sweep.sh (UDP). The matrix is 7 AQMs × 3 CCAs = 21
# runs, ~10 min of iperf3 time.
#
# Env overrides:
#   OUTDIR    output dir (default /tmp/qdisc_runs)
#   DURATION  iperf3 -t per run (default 25, spans the t=22s route change)
#   CCAS      space-separated CCA list (default "reno cubic bbr")
#   QDISC_FILTER  optional grep filter on qdisc name

set -uo pipefail

OUTDIR="${OUTDIR:-/tmp/qdisc_runs}"
DURATION="${DURATION:-25}"
CCAS=( ${CCAS:-reno cubic bbr} )
QDISC_FILTER="${QDISC_FILTER:-}"

mkdir -p "$OUTDIR"

# Ensure tcp_bbr is loadable; safe to ignore failure (may be built-in).
modprobe tcp_bbr 2>/dev/null || true

# Same AQM list as qdisc_ci_sweep.sh (UDP). Keep them in sync.
QDISCS=(
    "pfifo:pfifo limit 1000"
    "pfifo_head_drop:pfifo_head_drop limit 1000"
    "codel:codel target 5ms"
    "fq_codel:fq_codel limit 10240 target 5ms"
    "red:red limit 60000 min 5000 max 15000 avpkt 1000 burst 50"
    "pie:pie target 5ms tupdate 15ms"
    "fq_pie:fq_pie target 5ms"
)

# Sanity-check the CCAs the kernel actually offers. iperf3 -C <bad> would
# fail per-run; emit a warning here so the cause is obvious in the log.
if [ -r /proc/sys/net/ipv4/tcp_available_congestion_control ]; then
    AVAILABLE="$(cat /proc/sys/net/ipv4/tcp_available_congestion_control)"
    for cca in "${CCAS[@]}"; do
        if [[ " $AVAILABLE " != *" $cca "* ]]; then
            echo "::warning::CCA '$cca' is not in tcp_available_congestion_control ($AVAILABLE); runs will likely fail"
        fi
    done
fi

FAILED=()
for entry in "${QDISCS[@]}"; do
    name="${entry%%:*}"
    qdisc="${entry#*:}"
    if [ -n "$QDISC_FILTER" ] && [[ "$name" != *"$QDISC_FILTER"* ]]; then
        continue
    fi
    for cca in "${CCAS[@]}"; do
        if ! ./scripts/qdisc_ci_run_tcp.sh "$name" "$qdisc" "$cca" \
                --duration "$DURATION" --outdir "$OUTDIR"; then
            FAILED+=("${name}_${cca}")
        fi
    done
done

if [ "${#FAILED[@]}" -gt 0 ]; then
    echo "::warning::Failed TCP runs: ${FAILED[*]}"
fi

EXPECTED=$(( ${#QDISCS[@]} * ${#CCAS[@]} ))
ACTUAL=$(ls "$OUTDIR"/client_tcp_*.json 2>/dev/null | wc -l)
echo "TCP sweep complete: $ACTUAL of $EXPECTED runs produced output."
