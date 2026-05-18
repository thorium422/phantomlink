#!/bin/bash
# Drive scripts/qdisc_ci_run.sh across every AQM phantomlink supports today.
# Used by .github/workflows/qdisc-benchmark.yml; runnable manually too.
#
# Each AQM is wrapped in HTB at the scenario's bottleneck rate (this happens
# automatically whenever --qdisc-client/--qdisc-server is set). Individual
# failures are logged but do not abort the sweep; the absence of a
# `client_<name>.json` is reflected in the Markdown table.
#
# Env overrides:
#   OUTDIR    output dir (default /tmp/qdisc_runs)
#   DURATION  iperf3 -t per run (default 20)
#   TARGET    iperf3 -b offered load (default 200M)

set -uo pipefail

OUTDIR="${OUTDIR:-/tmp/qdisc_runs}"
DURATION="${DURATION:-20}"
TARGET="${TARGET:-200M}"

rm -rf "$OUTDIR"
mkdir -p "$OUTDIR"

# `<run-name>:<tc qdisc tokens>` pairs.
#
# The `_ecn` variants exercise the ECN-marking code path in each AQM. Under
# iperf3 UDP these will behave identically to the drop-mode siblings because
# iperf3's UDP sender doesn't set ECT — so the AQM has no ECN-capable packets
# to mark and falls back to dropping. They're kept for parity with the TCP
# sweep (where they do mark) and as a sanity check that adding `ecn` to the
# qdisc tokens doesn't accidentally change UDP behaviour.
QDISCS=(
    "pfifo:pfifo limit 1000"
    "pfifo_head_drop:pfifo_head_drop limit 1000"
    "codel:codel target 5ms"
    "codel_ecn:codel target 5ms ecn"
    "fq_codel:fq_codel limit 10240 target 5ms"
    "fq_codel_ecn:fq_codel limit 10240 target 5ms ecn"
    "red:red limit 60000 min 5000 max 15000 avpkt 1000 burst 50"
    "red_ecn:red limit 60000 min 5000 max 15000 avpkt 1000 burst 50 ecn"
    "pie:pie target 5ms tupdate 15ms"
    "pie_ecn:pie target 5ms tupdate 15ms ecn"
    "fq_pie:fq_pie target 5ms"
    "fq_pie_ecn:fq_pie target 5ms ecn"
)

FAILED=()
for entry in "${QDISCS[@]}"; do
    name="${entry%%:*}"
    qdisc="${entry#*:}"
    if ! ./scripts/qdisc_ci_run.sh "$name" "$qdisc" \
            --duration "$DURATION" --target "$TARGET" --outdir "$OUTDIR"; then
        FAILED+=("$name")
    fi
done

if [ "${#FAILED[@]}" -gt 0 ]; then
    echo "::warning::Failed qdisc runs: ${FAILED[*]}"
fi

echo "Sweep complete: $(ls "$OUTDIR"/client_*.json 2>/dev/null | wc -l) of ${#QDISCS[@]} runs produced output."
