# Architecture

fuckipv6 is an IPv6 overlay network. Clients create a local TUN interface and exchange raw IPv6 packets over a P2P UDP data plane. A coordinator provides client discovery, virtual address allocation, health cleanup, WebSocket signaling, and a signaling-channel relay fallback.

## Components

### common

`crates/common` contains shared protocol, config, crypto, and type definitions:

- `protocol.rs`: `SignalingMessage`, `P2pDatagram`, `DataPacket`, and control messages.
- `types.rs`: `PeerInfo`, NAT type labels, route entries, and connection quality helpers.
- `crypto.rs`: Ed25519 identity generation, persistence, signing, verification, and public-key fingerprint client IDs.
- `config.rs`: client and coordinator config structures plus validation/defaults.

### coordinator

`crates/coordinator` is the public rendezvous server.

- `signaling/server.rs` accepts WebSocket connections and handles registration, connect requests, heartbeats, and `RelayPacket` fallback.
- `signaling/session.rs` owns online sessions, virtual IPv6 allocation, peer snapshots, heartbeat timestamps, and coordinator traffic counters.
- `webui/mod.rs` exposes the HTML dashboard, REST APIs, logs API, and realtime WebSocket snapshots.
- `relay/server.rs` documents the planned dedicated relay service. Current relay fallback is implemented through the signaling WebSocket.

### client

`crates/client` is the node process.

- `tun/` creates and tears down OS TUN devices. Linux uses `ip tuntap`; macOS uses `utun`; Windows uses native embedded WinTun driver with pure Rust Windows IP Helper API for address/route configuration (zero external DLL and zero `netsh.exe` dependency).
- `p2p/connection.rs` runs the UDP P2P engine, endpoint probing, ping/pong, health checks, active endpoint selection, and fallback decisions.
- `signaling/client.rs` manages the coordinator WebSocket, registration, reconnect, re-register, and event delivery.
- `cli/commands.rs` wires TUN, P2P, signaling, stats, graceful shutdown, and operator commands.
- `config_manager.rs` manages saved multi-network profiles under the user config directory.
- `ipc.rs` exposes local runtime status for `status` and `peers`.

## Startup Flow

1. The coordinator starts the signaling listener, Web UI listener, heartbeat cleanup task, and virtual IP allocator.
2. The client loads config, initializes logging, loads or creates an Ed25519 identity, starts the UDP P2P engine, and connects to the coordinator.
3. The client sends `Register` with public key, P2P port, timestamp, and an Ed25519 signature over the registration challenge.
4. The coordinator verifies that `client_id` matches the public-key fingerprint and that the signature is valid.
5. The coordinator allocates a virtual IPv6 address, returns `RegisterAck`, and broadcasts `PeerOnline` to existing clients.
6. The client creates/configures the TUN interface and enters the packet forwarding loop.

## Data Path

Outbound packets:

1. An application sends IPv6 traffic to a virtual peer address.
2. The OS routes the packet into the TUN interface.
3. The client reads the raw IPv6 packet, extracts destination IPv6, and looks up a known peer.
4. If the peer has a healthy verified UDP endpoint, the packet is serialized as `P2pDatagram::Data(DataPacket)` and sent over UDP.
5. If P2P is unavailable or unhealthy, the packet is sent to the coordinator as `SignalingMessage::RelayPacket`.

Inbound packets:

1. UDP P2P `Data` packets or signaling `RelayPacket` events are decoded by the client.
2. The raw IPv6 payload is written back to the local TUN device.
3. The OS delivers it to the local application stack.

## P2P Health And Fallback

The current data plane uses UDP datagrams. Clients probe all advertised endpoints, respond to ping/pong messages, track RTT, recent probe loss, payload traffic samples, and remote quality reports. A peer is eligible for direct P2P only when the endpoint is verified and health thresholds are satisfied.

Fallback uses the signaling WebSocket. The coordinator routes `RelayPacket` messages by destination virtual IPv6 address. Coordinator traffic counters track signaling messages/bytes, relayed packet count, relayed payload bytes, and relay errors.

## Addressing

The coordinator allocates virtual IPv6 addresses from the configured pool, defaulting to `fd00::1000/120`. Client endpoints may be IPv4 or IPv6 socket addresses; the overlay payload itself is IPv6.

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

1. **Coordinator WebUI & API JWT Authentication**:
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
   - Clients perform concurrent RTT latency probing and connect to the optimal relay node.

## Current Boundaries

Implemented today:

- WebSocket signaling and cryptographic registration challenge authentication.
- Linux, macOS (`utun`), and Windows (embedded WinTun) dual-stack TUN drivers.
- WireGuard-grade ChaCha20-Poly1305 / X25519 end-to-end encryption.
- Direct dual-mode P2P (QUIC Stream Mesh + Encrypted Raw UDP).
- RFC 6886 NAT-PMP and UPnP IGD automatic router port mapping.
- Packet-level ACL filtering engine and coordinator admission blacklist.
- Distributed multi-relay mesh with latency-based concurrent selection.
- Coordinator WebUI management dashboard, REST API, logs API, and JWT authentication.
- Multi-network saved client profiles.
