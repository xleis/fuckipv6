# API Reference

This document describes the current wire protocol and coordinator HTTP APIs. Rust enum layouts are serialized with `bincode`; field names below document the logical schema, not a JSON encoding for the signaling protocol.

## Signaling WebSocket

Default listener: `0.0.0.0:6666`.

Transport:

- WebSocket binary messages.
- Payload encoding: `bincode` serialized `SignalingMessage`.
- Application-level heartbeat: `Heartbeat` / `HeartbeatAck`.

### Register

Client to coordinator.

Fields:

- `client_id`: public-key fingerprint string.
- `public_key`: Ed25519 public key bytes.
- `virtual_ip`: optional requested IPv6 address. The current coordinator allocates the address.
- `p2p_port`: UDP P2P listen port advertised to peers.
- `timestamp`: client timestamp in seconds.
- `signature`: Ed25519 signature over `register_challenge`.

The registration challenge is:

```text
"fuckipv6-register-v1" || client_id || public_key || p2p_port_be || timestamp_be
```

The coordinator verifies both the public-key fingerprint and the signature.

### RegisterAck

Coordinator to client.

Fields:

- `assigned_ip`: allocated primary virtual IPv6 address.
- `assigned_ips`: optional dual-stack virtual IPs (`ipv4` and `ipv6`).
- `network_prefix`: IPv6 prefix length.
- `ipv4_prefix`: optional IPv4 subnet prefix length.
- `peers`: peer list excluding the registering client.
- `relay_endpoint`: optional primary QUIC relay server endpoint (`addr`, `server_name`, `public_key_pin`).
- `relay_endpoints`: optional list of candidate distributed QUIC relay endpoints for optimal RTT selection.

### ConnectRequest

Client to coordinator.

Fields:

- `target_id`: requested peer client ID.

### ConnectInfo

Coordinator to requesting client.

Fields:

- `peer`: target peer info.

### ConnectNotify

Coordinator to target client.

Fields:

- `peer`: requesting peer info.

### PeerOnline

Coordinator broadcast when a client registers.

Fields:

- `peer`: new peer info.

### PeerOffline

Coordinator broadcast when a client disconnects or times out.

Fields:

- `client_id`: disconnected peer client ID.

### RelayPacket

Client to coordinator, then coordinator to destination client.

Fields:

- `src_ip`: source virtual IPv6 address.
- `dst_ip`: destination virtual IPv6 address.
- `payload`: raw IPv6 packet bytes from TUN.

The coordinator routes relay packets by `dst_ip`. If the destination is offline, it returns an error to the sender and increments the relay error counter.

### Heartbeat

Client to coordinator.

Fields:

- `timestamp`: client timestamp in seconds.

### HeartbeatAck

Coordinator to client.

Fields:

- `timestamp`: timestamp copied from `Heartbeat`.

### Error

Coordinator to client.

Fields:

- `code`: numeric error code.
- `message`: human-readable error.

Common codes:

- `401`: invalid registration or relay attempted before registration.
- `404`: requested peer not found.
- `500`: internal handler error.

## P2P UDP Datagram

Transport:

- UDP datagrams between client endpoints.
- Payload encoding: `bincode` serialized `P2pDatagram`.

### Data

Carries TUN payload.

Fields:

- `src_ip`: source virtual IPv6 address.
- `dst_ip`: destination virtual IPv6 address.
- `payload`: raw IPv6 packet bytes.
- `timestamp`: sender timestamp in microseconds.

### Ping

Endpoint probing and RTT measurement.

Fields:

- `src_ip`: sender virtual IPv6 address.
- `timestamp`: sender timestamp in microseconds.

### Pong

Response to `Ping`.

Fields:

- `src_ip`: responder virtual IPv6 address.
- `timestamp`: original ping timestamp.

### Control

P2P control channel.

Fields:

- `src_ip`: sender virtual IPv6 address.
- `message`: one of:
  - `Ping { seq }`
  - `Pong { seq }`
  - `RouteUpdate { routes }`
  - `QualityReport { rtt_us, loss_rate, bandwidth_bps }`

`QualityReport` is used by peer health tracking.

## PeerInfo

Shared peer descriptor.

Fields:

- `client_id`: public-key fingerprint string.
- `virtual_ip`: virtual IPv6 address.
- `public_key`: Ed25519 public key bytes.
- `endpoints`: socket addresses advertised for P2P.
- `nat_type`: one of `Open`, `FullCone`, `Restricted`, `PortRestricted`, `Symmetric`, `Unknown`.

Clients run STUN/RFC5780 NAT classification before registration and report the result. If no configured STUN server can provide enough evidence, the client reports `Unknown`.

## Coordinator HTTP API

Default listener: `0.0.0.0:8080`.

### Authentication

When coordinator authentication is enabled (`auth.enabled = true` or `admin_password` is set), protected endpoints require a JWT bearer token via `Authorization: Bearer <token>` or URL query param `?token=<token>`.

### POST /api/auth/login

Authenticates administrator credentials and returns a signed JWT token.

Request:
```json
{
  "username": "admin",
  "password": "your_password"
}
```

Response:
```json
{
  "success": true,
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "error": null,
  "expires_in": 86400,
  "auth_required": true
}
```

### GET /api/blacklist

Returns the list of currently blacklisted client IDs or public key fingerprint prefixes.

Response:
```json
["banned-client-1", "revoked-fingerprint-prefix"]
```

### POST /api/blacklist/ban

Adds a client ID or public key fingerprint prefix to the coordinator admission blacklist and immediately terminates active sessions.

Request:
```json
{
  "client_id": "malicious-node-id"
}
```

### POST /api/blacklist/unban

Removes an entry from the coordinator admission blacklist.

Request:
```json
{
  "client_id": "malicious-node-id"
}
```

### GET /api/config

Returns active runtime virtual network allocation and prefix configurations.

### POST /api/config

Hot-updates runtime virtual network mode and CIDR address pools.

### GET /

Returns the embedded HTML dashboard.

### GET /api/status

Returns service status and coordinator traffic counters.

Example shape:

```json
{
  "service": "fuckipv6-coordinator",
  "online_clients": 2,
  "traffic": {
    "uptime_secs": 3600,
    "signaling_rx_messages": 120,
    "signaling_rx_bytes": 4096,
    "signaling_tx_messages": 128,
    "signaling_tx_bytes": 8192,
    "relay_packets": 10,
    "relay_payload_bytes": 12800,
    "relay_errors": 1
  }
}
```

### GET /api/peers

Returns an array of online peer snapshots.

Example shape:

```json
[
  {
    "client_id": "peer-id",
    "virtual_ip": "fd00::1001",
    "endpoints": ["203.0.113.10:51820"],
    "nat_type": "Unknown",
    "connected_secs": 60,
    "idle_secs": 3
  }
]
```

### GET /api/logs

Query parameters:

- `service`: `coordinator`, `client`, a bare service name, or a full `.service` name. Default is `fuckipv6-coordinator.service`.
- `lines`: number of lines to return, clamped to `1..=500`. Default is `100`.

Linux uses `journalctl -u <service> -n <lines> --no-pager`. Other platforms return an explanatory error.

Example shape:

```json
{
  "service": "fuckipv6-coordinator.service",
  "lines": ["Jun 08 12:00:00 host fuckipv6-coordinator[1]: started"],
  "error": null
}
```

### GET /ws

Upgrades to a WebSocket and sends a JSON snapshot every two seconds.

Snapshot shape:

```json
{
  "service": "fuckipv6-coordinator",
  "online_clients": 2,
  "peers": [],
  "traffic": {
    "uptime_secs": 3600,
    "signaling_rx_messages": 120,
    "signaling_rx_bytes": 4096,
    "signaling_tx_messages": 128,
    "signaling_tx_bytes": 8192,
    "relay_packets": 10,
    "relay_payload_bytes": 12800,
    "relay_errors": 1
  }
}
```

## Client Local IPC

Default listener: `127.0.0.1:6668`.

The IPC protocol is local JSON line request/response traffic used by CLI commands such as `status` and `peers`. It is intended for local operator tooling, not remote control.
