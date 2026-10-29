# Architecture

fuckipv6 is a dual-stack overlay network. Clients create a local TUN interface and exchange raw IPv4/IPv6 packets over a dual-mode QUIC/UDP P2P data plane. A coordinator provides discovery, virtual address allocation, health cleanup, WebSocket signaling, QUIC relay discovery, and signaling-channel fallback. Onion circuits, Kademlia DHT, and chunk transfer are integrated P2P infrastructure, but they are not yet complete user-facing products.

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

`crates/coordinator` is the public rendezvous server.

- `signaling/server.rs` accepts WebSocket connections and handles registration, connect requests, heartbeats, and `RelayPacket` fallback.
- `signaling/session.rs` owns online sessions, virtual IPv6 allocation, peer snapshots, heartbeat timestamps, and coordinator traffic counters.
- `webui/mod.rs` exposes the HTML dashboard, REST APIs, logs API, and realtime WebSocket snapshots.
- `relay/server.rs` runs the dedicated QUIC relay service with registration, forwarding, concurrency limits, and rate controls. The signaling WebSocket remains the final fallback path.

### client

`crates/client` is the node process.

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

1. **Coordinator WebUI & API JWT Authentication** (not applied to the client-local WebUI/API):
   - Lightweight RFC 2104 HMAC-SHA256 token manager with token expiration and claims.
   - Protected endpoints require Bearer Token authorization or token query param.
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
- WireGuard-grade ChaCha20-Poly1305 / X25519 end-to-end encryption.
- Direct dual-mode P2P (QUIC Stream Mesh + encrypted raw UDP) and QUIC/signaling relay fallback.
- RFC 6886 NAT-PMP and UPnP IGD automatic router port mapping.
- Packet-level ACL filtering and coordinator admission blacklist.
- Multi-relay selection at connection time using concurrent RTT/handshake probing.
- Coordinator WebUI/API with optional JWT, plus client CLI and local WebUI.
- The client web desktop entry uses a browser `--app` window when available and falls back to the default browser; it is not a Tauri application.
- Linux Exit Node forwarding through kernel IP forwarding, policy routing, and iptables/NAT rules.
- Multi-network saved profiles and symmetric `host`/`join` networking with an embedded coordinator.
- Smart routing (`MeshOnly`, `BypassMainland`, `GlobalProxy`) and full Chinese runtime logging.

### Infrastructure integrated, pending productization

- Three-hop onion circuit and layered encryption primitives.
- Kademlia DHT routing and storage primitives.
- Blake3 chunk hashes, chunk assembly, and transfer primitives.
- Complete CLI/WebUI workflows for creating circuits, browsing/publishing DHT records, or publishing/fetching files have not been implemented.

### Not productized

- Tauri native desktop integration.
- User-space NAT; the current Exit Node path is Linux kernel-based.
- Windows/macOS Exit Node parity with the Linux implementation.
- Repeatable real cross-host NAT, production WAN scale, independent security audit, and target-platform runtime validation.

## Verification Baseline

The repository quality gate expects `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and Linux musl plus Windows GNU compile checks. Real cross-host NAT, privileged TUN, Windows/macOS runtime, large-scale WAN load, and independent security audit remain pre-production validation work.
