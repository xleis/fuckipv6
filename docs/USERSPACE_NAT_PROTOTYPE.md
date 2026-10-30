# Userspace NAT Feasibility Prototype (NX-16)

- Status: **experimental feasibility prototype; NO-GO for production integration**
- Feature: `userspace-nat-prototype` (disabled by default)
- Core: `crates/client/src/userspace_nat_prototype/`

This prototype evaluates whether a bounded userspace IPv4 NAT tracker is a credible
fallback for platforms without the current Linux kernel forwarding/NAT path. It is
deliberately isolated from the node runtime. The feature only compiles the prototype;
no CLI, WebUI, GPUI, TUN, packet-forwarding or configuration path calls it. Disabling
the default-off feature removes the module from the production build.

It is not a NAT implementation yet. It tracks and validates flows and chooses external
ports, but it does not rewrite packets, update checksums, move bytes, expose a network
interface or forward traffic.

## Implemented prototype behavior

- Strict IPv4/TCP/UDP parsing with IPv4 and transport checksum validation.
- Rejection of truncation, invalid lengths, headers and checksums, zero ports,
  non-unicast addresses, unsupported protocols and IPv4 fragments.
- Bounded TCP and UDP flow state with deterministic millisecond deadlines.
- TCP `SynSent -> Established -> Closing` lifecycle, FIN linger timeout and immediate
  RST removal.
- UDP bidirectional mapping timeout.
- Bounded external-port allocator with deterministic round-robin allocation and reuse
  of released ports before advancing.
- Per-source and global active-flow limits. A full table rejects a new flow instead of
  evicting a live mapping.
- Fail-closed rejects for unknown inbound packets, invalid TCP transitions, port
  exhaustion, clock rollback and malformed packets.
- Introspection counters and flow snapshots without secrets or packet payloads.
- No third-party dependencies in the prototype core.

All packet entry points require `&mut NatTable`; callers must serialize access. This
single-owner model avoids unmeasured locking and makes ordering deterministic.

## Default resource budget

| Resource | Default | Enforcement |
|---|---:|---|
| Active flows globally | 4,096 | New flows fail with `CapacityExceeded` |
| Active flows per internal source IPv4 address | 256 | New flows fail with `SourceFlowLimit` |
| External ports | 40,000-60,999 | Allocation fails with `PortExhausted` |
| TCP SYN timeout | 60 s | Deadline index |
| TCP established timeout | 300 s | Deadline index |
| TCP closing timeout | 30 s | Deadline index |
| UDP timeout | 60 s | Deadline index |
| Logical memory accounting | 64 KiB + 512 B/flow | Configuration and insertion checks |

At the default 4,096-flow ceiling, the logical estimate is **2,162,688 bytes
(2.0625 MiB)** against an 8 MiB logical budget. The 512 B/flow figure reserves space
for the flow record plus all three indexes, per-source accounting and port ownership.
It is a conservative accounting unit, not an allocator-accurate RSS measurement. No
resident-memory result is claimed.

The provisional performance gate for any continuation is one fixed, quiet CPU with
1500-byte packets: at least 1 Gbit/s single-core forwarding of already-validated
packets and p99 validation-plus-lookup overhead no greater than 10 microseconds at the
default flow ceiling. These thresholds have **not** been measured. The prototype has no
packet rewrite, checksum update, I/O loop or WireGuard comparison, so it cannot support
a performance claim.

## Deterministic verification

Run:

```bash
cargo test -p fuckipv6-client --features userspace-nat-prototype userspace_nat_prototype
cargo clippy -p fuckipv6-client --features userspace-nat-prototype --all-targets -- -D warnings
cargo check -p fuckipv6-client --no-default-features
```

The tests cover TCP setup/FIN/RST, UDP traffic and expiry, exact timeout boundaries,
port reuse, source/global capacity rejection without live-flow eviction, port
exhaustion, malformed checksums, fragments, unknown inbound packets, invalid TCP starts,
clock rollback and the logical memory ceiling. They are deterministic and use synthetic
packet bytes; they do not measure wall-clock performance.

## Fail-closed invariants

1. Malformed or unsupported packets never create or refresh a mapping.
2. Inbound packets are accepted only through an existing `(remote endpoint, external
   port, protocol)` mapping.
3. A capacity, source-limit, memory or port failure does not evict a live flow and does
   not consume partial state.
4. Time only moves forward. A rollback is an error, not an invitation to reset state.
5. Invalid TCP lifecycle transitions are rejected. New TCP mappings require an
   outbound SYN without ACK/FIN/RST.
6. Disabling `userspace-nat-prototype` compiles no prototype code and leaves all
   production paths unchanged.

## Known limitations and missing evidence

- IPv4 only; no IPv6 NAT behavior, ICMP, fragmented packets, PMTUD handling, hairpin,
  port preservation, NAT64, application gateways or endpoint-independent filtering.
- No packet buffer rewrite or checksum recomputation after source-port translation.
- No external-interface address model; inbound matching uses remote endpoint and
  external port only.
- No TCP sequence/window tracking, simultaneous open, half-close completion or RST
  validation against sequence state.
- No multiqueue/thread safety, sharded tables or lock-contention measurement.
- No evidence for allocation rate, CPU cycles per packet, throughput, p99 latency or
  RSS. The standard-library `BTreeMap`/`BTreeSet` choices may not meet the provisional
  gate.
- No interoperability, cross-platform TUN, security review or real-network testing.

## Decision

**NO-GO for production integration.** The logical behavior is bounded and fail-closed
under deterministic tests, but the evidence does not cover the packet rewrite path,
actual forwarding performance, resident memory, cross-platform behavior or security.
The current work is suitable only as a reference/benchmark harness input. It must not be
wired into Exit Node capability detection or presented as working userspace NAT.
