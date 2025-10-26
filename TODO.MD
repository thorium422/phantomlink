- TODO:
    - Make sure pacer only takes packets from the qdisc if it can consume them
    - Make sure qdisc only outputs packets if it's requested by the pacer

- Interesting qdiscs: 
    - headdrop/taildrop aqm 
    - codel
    - fq_codel
    - red
    - pie
    - l4s
    - ecn

- Edit congestion controller
    - cubic
    - bbr

- Setup automation for testing combinations (parralelization)    
    - gnu-parallel

- Things to measure:
   - Throughput (measure w iperf --json)
   - One way delay/latency (measure w iperf)
   - tc -s can show snapshots ab qdisc (measure every n time)
   - tcpdump -i to track packets