#!/bin/bash
# Drive scripts/qdisc_ci_run.sh across every AQM phantomlink supports today.
# Used by .github/workflows/qdisc-benchmark.yml; runnable manually too.
#
# Each AQM is wrapped in HTB at the scenario's bottleneck rate via the
# --qdisc-*-shaper htb CLI flag. Individual failures are logged but do not
# abort the sweep; the absence of a `client_<name>.json` is reflected in the
# Markdown table.
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

# `<run-name>:<tc qdisc tokens>` pairs. Covers the TODO.MD list except for
# headdrop variants (pfifo_head_drop) and L4S/ECN-only AQMs that need an
# ECN-capable sender. Tokens mirror the examples in docs/running-qdiscs.md.
QDISCS=(
    "pfifo:pfifo limit 1000"
    "codel:codel target 5ms"
    "fq_codel:fq_codel limit 10240 target 5ms"
    "red:red limit 60000 min 5000 max 15000 avpkt 1000 burst 50"
    "pie:pie target 5ms tupdate 15ms"
    "fq_pie:fq_pie target 5ms"
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
