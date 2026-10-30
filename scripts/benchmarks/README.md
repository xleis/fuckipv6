# NX-17 Reproducible Dataplane Benchmark

This directory is the evidence harness for NX-17. It does not claim that
fuckipv6 is faster than WireGuard. It records an auditable comparison and exits
non-zero unless the predeclared thresholds are met.

## Existing entry points audited

- `scripts/bench-dataplane.sh` creates the Linux network namespaces, runs
  fuckipv6, kernel WireGuard and wireguard-go with the same MTU and iperf3
  traffic, and saves raw iperf3/perf files plus `results.jsonl`.
- `scripts/bench-summarize.py` parses raw iperf3/perf output into per-run records
  and medians. It does not enforce a comparison policy.
- `scripts/bench-compare.py` covers same-host Criterion regressions. It is not an
  end-to-end tunnel comparison and is not a substitute for this harness.
- `SECURITY_PERFORMANCE_TODO.md` defines the 105% dataplane target and requires
  same-host, same-MTU and pinned-CPU comparisons. This harness makes that target
  executable rather than implied.

## What the harness adds

`run.py` wraps the existing dataplane runner and adds:

- exact kernel release and wireguard-go version/SHA-256 pinning;
- a quiet-host preflight (`load1` and sampled CPU busy percentage);
- required, disjoint CPU sets for client, server and control processes;
- a complete traffic matrix and a pre-run `plan.json` whose hash must still match
  at evaluation time;
- concurrent 100 Hz ICMPv6 probes through the tunnel during every iperf3 client
  run, with raw probe output preserved;
- a final gate that fails on throughput, p99, CPU cost, packet loss, missing
  samples or missing implementations;
- an explicit `INCOMPLETE` result when wireguard-go is intentionally absent.

Every result is loaded from files created by the run. The harness never invents
missing values or substitutes a zero.

## Requirements

The full run needs Linux, root, network-namespace/TUN support, and these tools:

`ip`, `iperf3`, `perf`, `ping`, `python3`, `taskset`, `wg`, `git`

It also needs release binaries for `fuckipv6` and `fuckipv6-coordinator`, and a
pinned `wireguard-go` binary. Kernel WireGuard is provided by the pinned kernel;
for reproducibility, boot the exact kernel image identified by `--expected-kernel`
on every baseline and candidate run.

Build the project binaries first:

```bash
cargo build --release -p fuckipv6-client -p fuckipv6-coordinator
```

Disable unrelated workloads. Where CPU frequency scaling is available, prefer
the `performance` governor and pass `--require-performance-governor`. The
harness records governors either way. It also fails if one-second sampled CPU
busy time exceeds 5% or `load1` exceeds 0.5 by default.

## Fixed CPU allocation

All three CPU lists are mandatory and must be disjoint. Example for a 16-vCPU
host:

```text
client 2-3    iperf3 and fuckipv6 client
server 4-5    iperf3 and fuckipv6 server
control 6     coordinator
```

The traffic generator and the measured tunnel share a namespace by design; the
current `bench-dataplane.sh` interface applies the same pinning to both. CPU cost
is therefore host-wide busy CPU per UDP sent packet (or per byte for TCP, because
iperf3 JSON does not report TCP packet counts). A quiet host and pinned CPU sets
are mandatory, but this remains an end-to-end system metric, not a claim of exact
per-process attribution.

## Traffic models

| Test | Load | Gate |
|---|---|---|
| `tcp_1flow` | unlimited TCP, one flow | throughput, p99, CPU/byte |
| `tcp_8flow` | unlimited TCP, eight flows | throughput, p99, CPU/byte |
| `udp_1420_saturated` | unlimited 1420-byte UDP | throughput, CPU/packet |
| `udp_64_saturated` | unlimited 64-byte UDP | throughput, CPU/packet |
| `udp_1420_controlled` | 50 Mbit/s 1420-byte UDP | throughput, p99, CPU/packet, loss |
| `udp_64_controlled` | 5 Mbit/s 64-byte UDP | throughput, p99, CPU/packet, loss |

The default run uses five repetitions and 30-second iperf3 streams with the first
two seconds omitted from iperf3 aggregation. Each iperf3 client run is wrapped by
a shim that starts a 10 ms ICMPv6 probe through the tunnel for the full stream;
the probe therefore includes the warm-up interval. The report uses nearest-rank
p99 from each run and the median of the five run p99 values. Probe runs below 200
RTT samples or above 1% loss fail the latency check. Saturated UDP tests do not
gate on p99 because the offered load intentionally exceeds capacity.

The raw files, full policy and manifest are retained so a reviewer can recompute
or challenge every aggregate.

## Predeclared pass policy

The defaults are written into `plan.json` before any traffic:

| Check | Pass rule |
|---|---|
| Throughput | fuckipv6 >= 105% of each comparator |
| p99 latency | fuckipv6 <= 100% of each comparator's p99 |
| CPU cost | fuckipv6 <= 100% of each comparator's CPU/UDP packet or CPU/TCP byte |
| Controlled UDP loss | both implementations <= 1% per run |
| Latency quality | >= 200 samples and <= 1% probe loss per run |
| Evidence matrix | all six tests, five runs, both comparators |

Comparators are the pinned kernel WireGuard release and the pinned wireguard-go
binary. Both checks use median aggregates and must pass independently. A single
failed model/comparator check makes the complete gate fail.

These values can be changed only through CLI arguments before collection; the
chosen policy and its SHA-256 are frozen in the output directory. The defaults
match the current 105% project target and are deliberately stricter than merely
matching WireGuard.

## Run

```bash
sudo python3 scripts/benchmarks/run.py \
  --expected-kernel "$(uname -r)" \
  --wireguard-go-bin /opt/wireguard-go/bin/wireguard-go \
  --expected-wireguard-go-version 'wireguard-go v0.0.20250522' \
  --expected-wireguard-go-sha256 '<64 lowercase hex characters>' \
  --client-cpus 2-3 \
  --server-cpus 4-5 \
  --control-cpus 6 \
  --require-performance-governor \
  --runs 5 \
  --duration 30 \
  --omit 2 \
  --output target/benchmarks/nx17-2026-10-30
```

Parameter validation and the exact plan can be inspected without root or network
changes using `--dry-run`. A dry run is not performance evidence.

Re-evaluate an existing evidence directory at any time:

```bash
python3 scripts/benchmarks/gate.py --output target/benchmarks/nx17-2026-10-30
```

Exit codes:

- `0`: complete matrix passed every predeclared check.
- `1`: invalid evidence or at least one threshold failed.
- `2`: explicit incomplete matrix, for example wireguard-go was skipped.

## Missing wireguard-go

By default, a missing wireguard-go binary is a hard preflight failure. For a
partial diagnostic run, pass `--allow-missing-wireguard-go`. The harness then
records the skip in `plan.json`, collects only the available implementations,
and exits `2` with `status: incomplete`. It never treats the missing comparator
as a pass and never fabricates a result.

Example:

```bash
sudo python3 scripts/benchmarks/run.py \
  --impls 'fuckipv6 wireguard-kernel' \
  --allow-missing-wireguard-go \
  --expected-kernel "$(uname -r)" \
  --client-cpus 2-3 --server-cpus 4-5 --control-cpus 6 \
  --output target/benchmarks/nx17-partial
```

## Output layout

The output directory contains:

- `plan.json` and `plan.sha256`: pre-run policy, versions, CPU sets and command;
- `environment.json`: kernel, CPU, load, governors, tools and kernel WG details;
- `metadata.json`, `lscpu.txt`: metadata emitted by the existing runner;
- `results.jsonl`, `summary.csv`: per-run iperf3 metrics and medians;
- `raw/`: untouched iperf3 JSON and perf CSV files;
- `latency/`: raw ICMP probe stdout/stderr per run;
- `latency-invocations.jsonl`: mapping from each iperf3 invocation to its probe;
- `logs/`, `harness.log`: implementation, perf and orchestration logs;
- `gate-report.json`: every ratio, threshold, failure and incomplete reason.

## Scope and limitations

The p99 metric is tunneled ICMPv6 echo tail latency under load. It is not
application request latency, handshake latency, rekey latency or a substitute for
a full protocol profile. CPU cost comes from host-wide `/proc/stat` busy time,
with `perf stat -a` counters retained as supporting raw data. These limitations
must be stated with any result.

The harness produces evidence, not a superiority claim. Only a complete passing
report, reproducible on documented hardware and kernel images, can support a
comparison statement. It does not replace independent performance review,
cross-platform evidence or security audit.
