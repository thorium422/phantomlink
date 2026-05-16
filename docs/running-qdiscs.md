# Running phantomlink with different qdiscs

This branch (`add-qdisc-support-clean`) adds an extra pair of veth devices
inside the `pl_link` namespace and attaches a `tc` qdisc of your choice to
each direction. Traffic from the client/server flows through that qdisc
before reaching the phantomlink pacer that emulates the LEO link
(delay + bottleneck rate from the scenario CSV).

So the path for one direction looks like:

```
client_ns  --veth-->  pl_link_ns  --[pqueueN_in : tc qdisc]-->  pqueueN_out
                                                                    |
                                                          phantomlink pacer
                                                          (delay + btlbw from CSV)
                                                                    |
                                                              --veth--> server_ns
```

`pqueue0` is the client→server direction; `pqueue1` is server→client. The
qdisc is attached to the `_in` side of each pair (see
`src/phork/namespace.rs:159`).

## CLI surface

`phantomlink setup` accepts two flags. Each takes the *raw tokens* you'd
pass to `tc qdisc add dev <iface> root ...`:

```
phantomlink setup \
    --qdisc-client "<tc qdisc tokens>" \
    --qdisc-server "<tc qdisc tokens>"
```

Defaults (both sides): `pfifo limit 1000`.

The tokens are space-delimited; quote the whole string so your shell
passes it as one argument. Examples:

```
sudo phantomlink setup -c "pfifo limit 1000"
sudo phantomlink setup -c "fq_codel limit 10240 target 5ms"
sudo phantomlink setup -c "codel limit 1000 target 5ms interval 100ms ecn" \
                       -s "codel limit 1000 target 5ms interval 100ms ecn"
```

If you only set `-c`, the server-side falls back to the `pfifo limit 1000`
default — keep that in mind when interpreting results.

## End-to-end smoke test

`scripts/run.sh` does setup → start → iperf3 (server + client) → teardown
in one shot, with the scenario from `examples/input.csv`. It only wires
the client side; if you need both sides, call the binary directly.

```
# build once
cargo build

# run with a specific qdisc on the client side
sudo ./scripts/run.sh 'pfifo limit 1000'
sudo ./scripts/run.sh 'fq_codel limit 10240 target 5ms'
sudo ./scripts/run.sh 'codel limit 1000 target 5ms interval 100ms ecn'
```

It prints the captured iperf3 client and server output at the end and
runs `phantomlink teardown` for you.

### Doing it by hand (both directions, longer experiments)

```
sudo ./target/debug/phantomlink setup \
    -c "fq_codel limit 10240 target 5ms" \
    -s "fq_codel limit 10240 target 5ms"

sudo ./target/debug/phantomlink start examples/input.csv &

sudo ./target/debug/phantomlink exec server iperf3 -s --port 5000 \
    > server.json 2>&1 &

sudo ./target/debug/phantomlink exec client \
    iperf3 -c 192.168.66.2 --port 5000 -t 60 --json > client.json

sudo ./target/debug/phantomlink teardown
```

## Inspecting the qdisc while a run is in flight

The qdisc lives inside the `pl_link` namespace on `pqueue{0,1}_in`. From
another terminal:

```
sudo ip netns exec pl_link tc -s qdisc show dev pqueue0_in
sudo ip netns exec pl_link tc -s qdisc show dev pqueue1_in
```

Loop it for a time series:

```
while sleep 0.5; do
    date +%s.%N
    sudo ip netns exec pl_link tc -s qdisc show dev pqueue0_in
done > qdisc_stats.txt
```

`tcpdump` works the same way — attach to either the `_in` or `_out` side
to see traffic before or after the qdisc:

```
sudo ip netns exec pl_link tcpdump -i pqueue0_in  -w pre_qdisc.pcap
sudo ip netns exec pl_link tcpdump -i pqueue0_out -w post_qdisc.pcap
```

## Starter recipes (the qdiscs from TODO.MD)

> **Note:** with all six of these used as the *root* qdisc the way the
> `--qdisc-client` flag attaches them, they will look identical in any
> measurement you run — no drops, empty queue, throughput pinned to the
> scenario rate. That isn't a bug in the qdiscs; it's a consequence of
> where phantomlink puts them. See
> [Why the starter-recipe AQMs all look identical](#why-the-starter-recipe-aqms-all-look-identical-and-how-to-fix-it)
> below for the workaround.

All of these are available on this kernel — verified with
`ls /lib/modules/$(uname -r)/kernel/net/sched/`.

```
# tail-drop FIFO (baseline)
-c "pfifo limit 1000"

# CoDel
-c "codel limit 1000 target 5ms interval 100ms"
-c "codel limit 1000 target 5ms interval 100ms ecn"   # CoDel + ECN

# FQ_CoDel
-c "fq_codel limit 10240 flows 1024 target 5ms interval 100ms"
-c "fq_codel limit 10240 flows 1024 target 5ms interval 100ms ecn"

# RED
-c "red limit 1000000 min 30000 max 90000 avpkt 1000 burst 55 probability 0.02"
-c "red limit 1000000 min 30000 max 90000 avpkt 1000 burst 55 probability 0.02 ecn"

# PIE
-c "pie limit 1000 target 15ms tupdate 15ms alpha 2 beta 20"
-c "pie limit 1000 target 15ms tupdate 15ms alpha 2 beta 20 ecn"

# FQ_PIE (L4S-friendly)
-c "fq_pie limit 10240 flows 1024 target 15ms tupdate 15ms ecn"
```

For L4S specifically you typically pair an ECN-capable AQM (CoDel/PIE
with `ecn`) with a DualPI2 / L4S-aware classifier. The mainline `pie`
qdisc has an `ecn` flag; the dedicated DualPI2 isn't shipped here.
Confirm with `tc qdisc add dev … help` what your iproute2 understands.

## Why the starter-recipe AQMs all look identical (and how to fix it)

If you sweep the six AQMs above through `--qdisc-client`/`--qdisc-server`
and run iperf3 against `examples/input.csv`, every one will report
essentially the same throughput, the same handful of retransmits, and a
mid-run `tc -s qdisc show` of `dropped 0, overlimits 0, backlog 0b 0p`
end-to-end. That holds whether you push TCP at the scenario rate or UDP
at 2.5× the scenario rate. The qdiscs aren't broken — they just never
get to do anything in the position phantomlink wires them into.

### Where the qdisc actually sits

```
client_ns ─veth─► pl_link_ns ─[pqueue0_in : tc qdisc]──► pqueue0_out
                                                              │
                                                       phantomlink pacer
                                                       (delay + btlbw from CSV)
                                                              │
                                                          ─veth─► server_ns
```

`pqueue0_in → pqueue0_out` is a veth pair *inside* the `pl_link`
namespace. A veth has no rate limit, so whatever the qdisc dequeues
moves to the other side at line rate. The **only** bottleneck in the
path is the phantomlink pacer downstream of `pqueue0_out`. Consequences:

- **With TCP:** the sender self-clocks against the pacer rate. Offered
  load arriving at the qdisc equals service rate at the qdisc, so the
  queue stays empty and no AQM logic ever fires. Retransmits in iperf3
  output are coming from the pacer's own drop policy, not the qdisc.
- **With UDP at, say, 200 Mbps against an 80 Mbps pacer:** the qdisc
  still sees a clean pipe — it enqueues at 200 Mbps and dequeues at 200
  Mbps into the next veth. The pacer drops 60% of the packets after the
  qdisc has already passed them through. iperf3 reports the loss; the
  qdisc reports `dropped 0`.

Only *shaping* qdiscs — ones that enforce a rate themselves, like `tbf`
or `htb` — engage as the root qdisc, because they create their own
bottleneck. Try this and you'll see the queue fill immediately:

```
sudo ./target/debug/phantomlink setup \
    -c "tbf rate 80mbit burst 32kb limit 1500000"
```

For the classless AQMs from TODO.MD to behave as AQMs, you need to put a
rate-limiting parent in front of them.

### The fix: wrap each AQM in HTB

Make HTB the root with a single class at the scenario's bottleneck rate,
and attach your AQM as a child of that class:

```
qdisc htb 1: root, default 0x10
└─ class htb 1:10 rate 80mbit ceil 80mbit
   └─ qdisc <your AQM> 10:
```

`phantomlink setup --qdisc-client` only takes a single root-qdisc token
string, so you build this tree by hand after `setup` runs. `scripts/
qdisc_udp_run.sh` automates the whole experiment — setup, qdisc rewire,
phantomlink start, iperf3 UDP overload, time-series `tc` polling,
teardown, summary.

### Reproducing the result

Requires a working phantomlink build (`cargo build`) and root for the
`tc`/`ip netns` shell-outs. From the repo root:

```bash
cargo build

# Run the six TODO.MD AQMs, each wrapped in HTB@80mbit, with 200 Mbps
# UDP offered load for 30 seconds. Outputs go to /tmp/qdisc_runs/.
for q in "pfifo:pfifo limit 1000" \
         "codel:codel limit 1000 target 5ms interval 100ms" \
         "fq_codel:fq_codel limit 10240 flows 1024 target 5ms interval 100ms" \
         "red:red limit 1000000 min 30000 max 90000 avpkt 1000 burst 55 probability 0.02" \
         "pie:pie limit 1000 target 15ms tupdate 15ms alpha 2 beta 20" \
         "fq_pie:fq_pie limit 10240 flows 1024 target 15ms tupdate 15ms ecn"; do
    sudo ./scripts/qdisc_udp_run.sh "${q%%:*}" "${q#*:}"
done

python3 ./scripts/qdisc_udp_summarize.py
```

`qdisc_udp_run.sh` defaults to `--rate 80 --target 200M --duration 30
--outdir /tmp/qdisc_runs`. Pass `--no-htb` to attach the qdisc directly
as root (i.e., reproduce the no-engagement baseline). Each invocation
produces, in the output directory:

- `client_<name>.json` — full iperf3 client output
- `server_<name>.txt` — iperf3 server output
- `qdisc_<name>_ts.txt` — `tc -s qdisc show dev pqueue0_in` snapshots
  every 0.5 s for the full run, separated by `=== t=<sec> ===` markers
- `setup_<name>.log`, `start_<name>.log`, `teardown_<name>.log` —
  phantomlink command output
- `summary_<name>.txt` — one-line `sender / packets / lost / duration`

### Expected output

`qdisc_udp_summarize.py` joins iperf3's lost-percentage with the mid-run
(t≈15 s) tc state. Numbers within a few percentage points run-to-run:

```
qdisc        lost%     pkts  queue@t15   ldelay     drops  detail
-----------------------------------------------------------------
pfifo         60.9%   517942       997p        -    140449  drop_ovl=140449
codel         61.0%   517941       994p    135ms    140936  drop_ovl=136368 DROPPING
fq_codel      16.3%   517941      2279p        -      4555
red           61.1%   517942        58p        -    141844  early=141844
pie           62.6%   517941        55p        -    142062
fq_pie        61.3%   517940       204p        -    142831
```

What each line is telling you at t≈15 s under sustained 2.5× overdrive:

- **pfifo** — queue pegged at the 1000-packet limit; pure tail-drop.
- **codel** — also at the limit because the configured `limit 1000p` is
  too small at this overdrive; `ldelay 135 ms` is the head-of-queue
  sojourn time (27× codel's 5 ms target), and the qdisc is in
  `DROPPING` state. Bump `limit` or lower the offered rate and codel
  will hold sojourn near target.
- **fq_codel** — the apparent outlier: only 16% loss. Its default
  `limit 10240p` plus per-flow accounting absorbs the single UDP flow
  without aggressive dropping. Set `limit 1000` and it lands in the
  same ~61% band as the others; per-flow fairness only differentiates
  it under multi-flow load.
- **red** — queue stays at 58 packets because RED drops *early* before
  the queue fills. The `early=141844` counter is RED's own
  early-drop tally, distinct from tail-drops (`pdrop`, here zero).
- **pie** — same low-queue character as RED via predictive drop, just a
  different control law.
- **fq_pie** — per-flow + PIE; bigger working set than pie alone (204
  vs 55 packets) but same overall loss fraction.

All six match the expected 200→80 Mbps shaping (~60% loss) regardless
of AQM; the differentiation is in **queue depth and drop policy**, not
throughput.

### Why UDP and not TCP

TCP self-clocks against any bottleneck — including HTB — so even with
the HTB wrapper in place, a TCP iperf3 will quickly stabilize at an
empty AQM queue and you'll see the same "all AQMs look identical"
result as without the wrapper. UDP at a fixed rate above the HTB
ceiling is the simplest way to force the AQM to actually drop. To get
useful TCP measurements you'd compare *latency under load*
(e.g. ping/`irtt` in parallel) rather than throughput.

### Gotchas with the HTB wrapper

- `tc class add` prints `Warning: sch_htb: quantum of class 10010 is
  big. Consider r2q change.` — harmless for a single-class setup at
  80 Mbps.
- The ECN-capable AQM variants (`codel ... ecn`, `pie ... ecn`,
  `fq_pie ... ecn`) only mark instead of drop if the *sender* is
  ECN-capable. iperf3 UDP isn't, so the `ecn_mark` counter stays at 0
  in these runs. Run TCP with `sysctl -w net.ipv4.tcp_ecn=1` in the
  client namespace to see marks.
- `summary_<name>.txt` shows the *sender* rate (200 Mbps) regardless
  of how much was dropped. The receiver-side rate is in the same JSON
  under `end.sum` if you parse `bits_per_second` differently — for
  this experiment the lost-percentage is the more informative number.
- The script only rewires `pqueue0_in` (client → server). The
  reverse direction `pqueue1_in` still has the placeholder `pfifo
  limit 1` from `phantomlink setup`. For pure UDP one-way runs that
  doesn't affect the result; for TCP or for symmetric tests, mirror
  the same `tc` commands on `pqueue1_in`.

## Changing the congestion controller

Qdisc and CC are independent knobs. Set the CC inside whichever
namespace runs the sender; for the client→server iperf3 above, that's
`pl_client`:

```
sudo ip netns exec pl_client sysctl -w net.ipv4.tcp_congestion_control=cubic
sudo ip netns exec pl_client sysctl -w net.ipv4.tcp_congestion_control=bbr
```

(Check `sysctl net.ipv4.tcp_available_congestion_control` for what's
loaded.)

## Gotchas

- AQM qdiscs (pfifo, codel, fq_codel, red, pie, fq_pie) attached as the
  root via `--qdisc-client` never engage — the queue stays empty and
  drops stay at zero. See
  [Why the starter-recipe AQMs all look identical](#why-the-starter-recipe-aqms-all-look-identical-and-how-to-fix-it)
  for what's happening and how to wrap them in HTB.
- `phantomlink setup` refuses to run if a previous run left namespaces
  around. If something crashes mid-run, `sudo phantomlink teardown` first.
- Token splitting uses spaces only, so don't put commas or quotes inside
  the qdisc string. `fq_codel limit 10240 target 5ms` works;
  `fq_codel,limit,10240` does not.
- The `target/debug/phantomlink` binary is what `scripts/run.sh` uses.
  Use `cargo build --release` and point at `target/release/phantomlink`
  for measurement runs — debug builds will skew throughput.
- The `rust-toolchain.toml` was pinned to 1.87 but MSRV was bumped to
  1.88 in commit `e24fed5`; the pin is now 1.88 to match.
