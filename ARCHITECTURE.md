# Architecture

fuckipv6 is a dual-stack overlay network built around one product identity: the node. A node creates a local TUN interface and exchanges raw IPv4/IPv6 packets over a dual-mode QUIC/UDP P2P data plane. Discovery, virtual address allocation, health cleanup, WebSocket signaling, QUIC relay discovery, and signaling-channel fallback are network-service capabilities that a node can provide for a network or consume from another node.

Onion circuits, Kademlia DHT, and chunk transfer are integrated P2P infrastructure, but they are not yet complete user-facing products. The repository now publishes only the `fuckipv6` node executable; the former `fuckipv6-coordinator` user entry point has been removed and coordinator capabilities remain internal libraries. The top-level desktop entry now opens a real GPUI native shell backed by the shared node command bus and state store; it is not a browser, WebView, or `--app` window. The repository remains in a documented migration state because the GPUI business pages and automated GPUI/WebUI semantic parity tests are not complete.

## Unified Node Target Architecture (UN-01)

Status: accepted architecture baseline; implementation tracked by `UNIFIED_NODE_TODO.md`.

### Product model

| Term | Meaning |
|------|---------|
| **Node** | The only product identity. One `fuckipv6` binary represents one local device and owns its identity, configuration, lifecycle, logs, UI surfaces, and local control plane. |
| **Network** | An isolated overlay namespace with its own profile, TUN device, virtual addresses, routes, peers, and policies. One node may participate in several networks concurrently. |
| **Member** | A node's participation in one network. Membership is per-network state, not a process-global client/server mode. |
| **Network service** | A capability provided by a member for a network, such as discovery, address allocation, signaling, relay, policy, or Exit Node forwarding. These are roles, not separate products or privilege classes. |
| **Peer** | Another node's member identity as observed inside a network. |
| **Initiator / Responder** | Cryptographic handshake session roles, selected deterministically by session keys. They do not imply product identity, ownership, or a permanently privileged node. |

User-visible language uses **node**, **network**, **member**, **service role**, and **peer**. Protocol and TLS implementation types may continue to use `client` and `server` for compatibility, but those names must not leak into startup choices or product identity.

### Runtime layers

```text
                    one `fuckipv6` process (local node)
┌─────────────────────────────────────────────────────────────────────┐
│ UI adapters: GPUI (desktop) | WebUI HTTP/WS (headless) | CLI/IPC   │
├─────────────────────────────────────────────────────────────────────┤
│ Shared UI contract: capabilities, commands, snapshots, events,     │
│ validation errors, confirmations, and authorization requirements   │
├─────────────────────────────────────────────────────────────────────┤
│ Node command bus and supervisor                                    │
│ identity | config | lifecycle | logs | mode | multi-network state   │
├─────────────────────────────────────────────────────────────────────┤
│ Network manager                                                    │
│ network A actor | network B actor | ...                            │
│ members | addresses | peers | routes | policies | stats | roles    │
├─────────────────────────────────────────────────────────────────────┤
│ Per-network services and transports                                │
│ discovery/signaling | allocator | QUIC relay | Exit Node | TUN     │
└─────────────────────────────────────────────────────────────────────┘
```

The GPUI and WebUI adapters are peers at the presentation boundary. Neither adapter may own a separate network state machine or invoke transport modules directly. Both send typed commands to the node command bus and render snapshots/events from the shared contract.

### State ownership

- **Node-global state**: device identity, installed configuration references, runtime mode, lifecycle, local control endpoint, logs, and update/version information.
- **Per-network state**: membership, display name, TUN identity, virtual addresses, peers, topology, routes, relay selection, Exit Node role/selection, ACL/policy, reconnect state, and traffic counters.
- **Ephemeral UI state**: selected tab, modal visibility, drafts, and scroll position only. Ephemeral UI state must not become a second source of truth for node or network behavior.
- **Durable writes**: all persistent mutations go through one command implementation with one validator. UI adapters may pre-validate for responsiveness, but the command layer remains authoritative.

### Compatibility and removal boundary

| Area | Decision |
|------|----------|
| `common` protocol, crypto, routing, and wire types | Retain. Technical `client`/`server` identifiers may remain where they describe protocol peers or preserve wire/config compatibility. |
| `coordinator` library capabilities | Retain as internal network-service actors. Its standalone user entry point has been removed. |
| `fuckipv6` binary and node runtime | Evolve into the sole executable. Existing commands may remain as command verbs during migration, but not as client/server program choices. |
| Existing CLI/TUI/IPC | Route through the shared command bus and shared snapshots; keep as non-GUI adapters. |
| Browser `--app` desktop entry | Remove from the desktop path. It may remain only as an explicitly selected WebUI fallback, never as GPUI parity. |
| WebUI business logic and private state | Migrate to shared commands/snapshots, then delete duplicate mutation paths and DTO semantics. |
| `fuckipv6-coordinator` binary, installers, and services | Retired. Single-binary packaging and workspace-target checks prevent its reintroduction. |
| User-facing “client/server/coordinator” identity | Remove. Keep only node, network, member, service role, and the explicitly technical protocol session roles. |

### Non-negotiable acceptance rules

1. A release exposes one user entry point, `fuckipv6`.
2. Starting a node does not require choosing client or server mode.
3. Creating a network and joining a network are node commands, not separate programs.
4. GPUI starts without a browser in desktop mode; WebUI is the automatic headless surface.
5. GPUI and WebUI declare capabilities from one manifest and are tested for page, command, field, validation, confirmation, permission, and error parity.
6. Multi-network and multiple service roles are represented per network; a node is not globally promoted to a server role.
7. Single-binary, parity, packaging, and cross-platform gates must pass before the migration is declared complete.

## Components

### common

`crates/common` contains shared protocol, config, crypto, and type definitions:

- `protocol.rs`: `SignalingMessage`, `P2pDatagram`, `DataPacket`, and control messages.
- `types.rs`: `PeerInfo`, NAT type labels, route entries, and connection quality helpers.
- `crypto.rs`: Ed25519 identity generation, persistence, signing, verification, and public-key fingerprint client IDs.
- `config.rs`: client and coordinator config structures plus validation/defaults.
- `routing.rs`: embedded CN-CIDR routing rules and routing-mode evaluation.
- `dht.rs`, `onion.rs`, `chunk.rs`: integrated DHT routing/storage, layered circuit, and chunk-transfer primitives; CLI/WebUI product workflows are still pending.

### coordinator

`crates/coordinator` is the internal network-service library. It has no user-facing executable.

- `signaling/server.rs` accepts WebSocket connections and handles registration, connect requests, heartbeats, and `RelayPacket` fallback.
- `signaling/session.rs` owns online sessions, virtual IPv6 allocation, peer snapshots, heartbeat timestamps, and coordinator traffic counters.
- `webui/mod.rs` exposes the HTML dashboard, REST APIs, logs API, and realtime WebSocket snapshots.
- `relay/server.rs` runs the dedicated QUIC relay service with registration, forwarding, concurrency limits, and rate controls. The signaling WebSocket remains the final fallback path.

### client

`crates/client` builds the `fuckipv6` node process. Its crate name is retained for compatibility.

- `tun/` creates and tears down OS TUN devices. Linux uses `ip tuntap`; macOS uses `utun`; Windows uses native embedded WinTun driver with pure Rust Windows IP Helper API for address/route configuration (zero external DLL and zero `netsh.exe` dependency).
- `p2p/connection.rs` runs the encrypted UDP engine, endpoint probing, ping/pong, health checks, and fallback decisions.
- `p2p/quic.rs` provides the pinned TLS 1.3 QUIC mesh and multiplexed stream transport.
- `p2p/overlay.rs` dispatches DHT, onion, chunk-transfer, and direct-message datagrams inside the P2P engine.
- `routing/` implements smart routing and Exit Node selection.
- `signaling/client.rs` manages the coordinator WebSocket, registration, reconnect, re-register, and event delivery.
- `symmetric/mod.rs` embeds the coordinator actor for symmetric `host`/`join` networks.
- `cli/commands.rs` wires TUN, P2P, signaling, stats, graceful shutdown, and operator commands.
- `config_manager.rs` manages saved multi-network profiles under the user config directory.
- `ipc.rs` exposes local runtime status for `status` and `peers`.

## Startup Flow

1. The coordinator starts the signaling listener, Web UI listener, heartbeat cleanup task, and virtual IP allocator.
2. The client loads config, initializes logging, loads or creates an Ed25519 identity, starts the dual-mode UDP/QUIC P2P engine, and connects to the coordinator.
3. The client sends `Register` with public key, P2P port, timestamp, and an Ed25519 signature over the registration challenge.
4. The coordinator verifies that `client_id` matches the public-key fingerprint and that the signature is valid.
5. The coordinator allocates virtual IPv4/IPv6 addresses, returns `RegisterAck` with relay information, and broadcasts `PeerOnline` to existing clients.
6. The client creates/configures the TUN interface and enters the packet forwarding loop.

## Data Path

Outbound packets:

1. An application sends IPv4 or IPv6 traffic to a virtual peer or routed destination.
2. The OS routes the packet into the TUN interface.
3. The client reads the packet and applies ACL and smart-routing rules.
4. Mesh traffic uses a healthy pinned QUIC stream when available, otherwise the encrypted UDP path.
5. Routed traffic may use a selected Exit Node; non-direct traffic falls back to a QUIC relay and finally to signaling `RelayPacket`.

Inbound packets:

1. QUIC stream frames, UDP P2P datagrams, QUIC relay frames, or signaling `RelayPacket` events are decoded by the client.
2. The authenticated raw payload is written back to the local TUN device.
3. The OS delivers it to the local application stack.

## P2P Health And Fallback

The direct data plane supports pinned QUIC streams and encrypted raw UDP. Clients probe all advertised endpoints, respond to ping/pong messages, track RTT, recent probe loss, payload traffic samples, and remote quality reports. A peer is eligible for direct P2P only when the endpoint is verified and health thresholds are satisfied.

Fallback first selects a healthy QUIC relay by measured latency. If no QUIC relay is usable, the signaling WebSocket routes `RelayPacket` messages by destination virtual address. Counters track signaling messages/bytes, relayed packet count, relayed payload bytes, and relay errors.

## Addressing

The coordinator allocates dual-stack virtual addresses. IPv6 defaults to `fd00::1000/120`; IPv4 uses the configured allocation pool. Client endpoints may also use IPv4 or IPv6 socket addresses.

## Observability

Client-side observability:

- `fuckipv6 status`: local IPC snapshot including TUN, P2P, signaling, errors, and peer health.
- `fuckipv6 peers`: per-peer endpoint and quality details.
- `fuckipv6 route`: IPv6 route table view.
- `fuckipv6 logs`: systemd journal or file logs.

Coordinator-side observability:

- `/`: Web dashboard with online peers, topology, traffic metrics, and logs.
- `/api/status`: service status and traffic counters.
- `/api/peers`: online peer snapshots.
- `/api/logs`: Linux systemd journal tail for coordinator or client services.
- `/ws`: realtime snapshot stream every two seconds.

## Security & Governance

1. **Node control-plane JWT authentication**:
   - Lightweight RFC 2104 HMAC-SHA256 token manager with token expiration and claims.
   - Coordinator and local-node protected REST endpoints require Bearer Token authorization; browser WebSocket upgrades may use a token query parameter.
   - The local WebUI/API binds to `127.0.0.1` by default and supports explicit IPv6 loopback. Non-loopback binding requires a separate danger switch and configured administrator password, otherwise startup fails closed.
   - The local IPC endpoint also binds to loopback by default. It has no authentication handshake, so non-loopback IPC binding remains rejected even when the danger switch is present.
   - Zero-dependency login flow supporting admin passwords and dynamic secret rotation.

2. **Access Control Lists (ACL)**:
   - Declarative policy rules with action (`allow`/`deny`), direction (`inbound`/`outbound`/`both`), protocols (`tcp`/`udp`/`icmp`/`icmpv6`/`any`), CIDR filtering, and port ranges.
   - Enforced at the boundary between TUN and P2P/relay.

3. **Admission Blacklist**:
   - Coordinator registration gate checks client ID and BLAKE3 public key fingerprints.
   - Immediate session termination and disconnection of banned nodes.

## Advanced NAT Traversal & Transport

1. **Router Port Mapping**:
   - Automatic RFC 6886 NAT-PMP gateway UDP port mapping.
   - UPnP IGD WANIPConnection SOAP XML port mapping.
   - Mapped external endpoints prioritized as candidate endpoints for direct P2P.

2. **Dual-Mode Data Plane (QUIC + UDP)**:
   - Direct P2P QUIC mesh transport with pinned TLS 1.3 certificates.
   - Multiplexed bidirectional streams with automatic zero-overhead fallback to encrypted raw UDP datagrams.

3. **Multi-Relay Mesh & Optimal Selection**:
   - Coordinator advertises distributed QUIC relay clusters in `RegisterAck`.
   - Clients perform concurrent RTT/handshake probing at connection time and connect to the selected relay; this is not continuous migration or dynamic load balancing.

## Current Boundaries

### Directly usable

- WebSocket signaling and cryptographic registration challenge authentication.
- Linux, macOS (`utun`), and Windows (embedded WinTun) dual-stack TUN drivers, subject to target-platform runtime validation.
- Secure Session v2 classic is wired into the production P2P data path with ephemeral X25519, Ed25519 authentication, bidirectional key confirmation, counter nonces, a replay window, and fail-closed handling. QUIC Mesh, QUIC relay, and signaling relay carry the same v2 ciphertext. Independent cryptographic audit, production online rekey validation, and the post-quantum profile remain incomplete; this must not be described as WireGuard-equivalent or superior until those gates and performance baselines pass (see `SECURITY_PERFORMANCE_TODO.md`).
- Direct dual-mode P2P (QUIC Stream Mesh + encrypted raw UDP) and QUIC/signaling relay fallback.
- RFC 6886 NAT-PMP and UPnP IGD automatic router port mapping.
- Packet-level ACL filtering and coordinator admission blacklist.
- Multi-relay selection at connection time using concurrent RTT/handshake probing.
- Internal network-service WebUI/API with optional JWT, plus node CLI and a local WebUI that defaults to loopback-only exposure.
- The top-level desktop entry runs a real GPUI native shell with a dark theme, manifest-driven navigation, shared snapshot subscription, command submission, error banners, and timeout-bounded shutdown cleanup. The `--webui` path remains a browser-hosted surface; runtime subcommands that historically selected the desktop adapter still use the legacy WebUI adapter.
- Linux Exit Node forwarding through kernel IP forwarding, policy routing, and iptables/NAT rules.
- Multi-network saved profiles and symmetric `host`/`join` networking with an embedded coordinator.
- Smart routing (`MeshOnly`, `BypassMainland`, `GlobalProxy`) and full Chinese runtime logging.

### Infrastructure integrated, pending productization

- Three-hop onion circuit and layered encryption primitives.
- Kademlia DHT routing and storage primitives.
- Blake3 chunk hashes, chunk assembly, and transfer primitives.
- Complete CLI/WebUI workflows for creating circuits, browsing/publishing DHT records, or publishing/fetching files have not been implemented.

### Not productized

- GPUI business pages/action forms for the 11-page manifest, automated GPUI/WebUI semantic parity tests, and cross-platform GPUI runtime validation.
- User-space NAT; the current Exit Node path is Linux kernel-based.
- Windows/macOS Exit Node parity with the Linux implementation.
- Repeatable real cross-host NAT, production WAN scale, independent security audit, and target-platform runtime validation.

## Verification Baseline

The repository quality gate expects `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and Linux musl plus Windows GNU compile checks. Real cross-host NAT, privileged TUN, Windows/macOS runtime, large-scale WAN load, and independent security audit remain pre-production validation work.
