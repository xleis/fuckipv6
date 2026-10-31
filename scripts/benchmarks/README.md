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

- exact kernel release, clean source commit/tree, tool hashes and wireguard-go
  version/SHA-256 pinning;
- a quiet-host preflight (`load1` and sampled CPU busy percentage);
- required, disjoint CPU sets for client, server and control processes;
- a complete traffic matrix and a pre-run `plan.json` whose hash must still match
  at evaluation time;
- concurrent 100 Hz ICMPv6 probes through the tunnel during every iperf3 client
  run, with raw probe output preserved;
- an `evidence.sha256` manifest covering every retained raw and summary file;
- a final gate that reloads raw iperf3 JSON, perf CSV and ping output, verifies
  their manifest hashes, recomputes every reported metric, and fails on
  throughput, p99, CPU cost, packet loss, missing samples or missing
  implementations;
- an explicit `INCOMPLETE` result when wireguard-go is intentionally absent.

Every result is loaded from files created by the run. Summary values that do not
match the raw evidence are rejected. The harness never invents missing values or
substitutes a zero.

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

Disable unrelated workloads. The harness records CPU frequency governors and
fails if one-second sampled CPU busy time exceeds 5% or `load1` exceeds 0.5 by
default. `--require-performance-governor` is available for stricter diagnostic
runs, but changing it makes the policy non-canonical and therefore non-passing.

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
is recomputed from the retained system-wide `perf stat` task-clock counter per UDP
sent packet (or per byte for TCP, because iperf3 JSON does not report TCP packet
counts). A quiet host and pinned CPU sets are mandatory, but this remains an
end-to-end system metric, not a claim of exact per-process attribution.

## Traffic models

| Test | Load | Gate |
|---|---|---|
| `tcp_1flow` | unlimited TCP, one flow | throughput, p99, CPU/byte |
| `tcp_8flow` | unlimited TCP, eight flows | throughput, p99, CPU/byte |
| `udp_1420_saturated` | unlimited UDP, 1372-byte iperf3 payload | throughput, CPU/packet |
| `udp_64_saturated` | unlimited UDP, 64-byte iperf3 payload | throughput, CPU/packet |
| `udp_1420_controlled` | 50 Mbit/s UDP, 1372-byte iperf3 payload | throughput, p99, CPU/packet, loss |
| `udp_64_controlled` | 5 Mbit/s UDP, 64-byte iperf3 payload | throughput, p99, CPU/packet, loss |

The `udp_1420_*` names refer to the inner tunnel packet size. With an MTU of
1420, inner IPv6 and UDP headers consume 48 bytes, so the iperf3 payload is
`1420 - 48 = 1372` bytes. Using a 1420-byte iperf3 payload creates a 1468-byte
inner packet, fragments it, and invalidates receiver-side UDP measurements.

UDP throughput is computed only from iperf3 `end.sum_received`. If the receiver
reports zero bytes while the sender reports packets, the sample is marked
`valid=false` with `invalid_reason=zero_received_payload`; the harness does not
fall back to the sender summary and the benchmark fails.

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

The defaults are the canonical `nx17-canonical-v1` policy. The policy object and
its SHA-256 are frozen in `plan.json`; the gate requires the exact canonical hash.
CLI arguments may still create diagnostic variants, but any changed threshold,
run count, duration, traffic model or probe setting is marked `custom` and can
never produce `passed`/exit `0`. The canonical defaults match the current 105%
project target and are deliberately stricter than merely matching WireGuard.

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
  --runs 5 \
  --duration 30 \
  --omit 2 \
  --output target/benchmarks/nx17-2026-10-30
```

Parameter validation and the exact plan can be inspected without root or network
changes using `--dry-run`. The gate also requires the plan's traffic, environment
and latency settings to match the canonical policy exactly. A dry run is not
performance evidence.

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

- `plan.json` and `plan.sha256`: canonical policy/hash, clean source commit/tree,
  binary and tool hashes, CPU sets and command;
- `evidence.sha256`: hashes of every retained evidence file, excluding the
  final gate report itself;
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
a full protocol profile. CPU cost comes from the retained system-wide `perf stat`
task-clock counter and is not exact per-process attribution. These limitations
must be stated with any result.

The harness produces evidence, not a superiority claim. Only a complete passing
report, reproducible on documented hardware and kernel images, can support a
comparison statement. It does not replace independent performance review,
cross-platform evidence or security audit.
