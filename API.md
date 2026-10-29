# API Reference

This document describes the current wire protocol, coordinator HTTP APIs, and client-local WebUI/control APIs. Rust enum layouts are serialized with `bincode`; field names below document the logical schema, not a JSON encoding for the signaling protocol.

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

## Client WebUI / Control API

The client starts an embedded Axum WebUI when `--gui` or headless mode is selected. The CLI default port is `8080`; override it with `--webui-port <PORT>`.

> **Security boundary:** this server currently has no authentication (`auth_required` is always `false`) and binds to `0.0.0.0`. Do not expose it directly to an untrusted network. Restrict it with a firewall, bind it only in a trusted environment, or place an authenticated reverse proxy in front of it. This API is separate from the coordinator API, where JWT can be enabled.

### Endpoint summary

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/` | Embedded client WebUI |
| `GET` | `/assets/webui.css` | Shared WebUI stylesheet |
| `GET` | `/api/status` | Current client and embedded-coordinator snapshot |
| `GET` | `/api/peers` | Peer snapshots |
| `POST` | `/api/peers/probe` | Run a real P2P probe against one peer |
| `GET` | `/api/config` | Current virtual-network allocation |
| `POST` | `/api/config` | Hot-update allocation while embedded coordinator is active |
| `GET` | `/api/blacklist` | Embedded-coordinator blacklist |
| `POST` | `/api/blacklist/ban` | Ban a client and return the updated blacklist |
| `POST` | `/api/blacklist/unban` | Unban a client and return the updated blacklist |
| `POST` | `/api/auth/login` | Compatibility login endpoint; no authentication is required |
| `GET` | `/api/logs` | Read runtime or system logs |
| `GET` | `/ws` | WebSocket snapshot stream |
| `POST` | `/api/connect` | Connect the client to a network |
| `POST` | `/api/disconnect` | Disconnect the running client |
| `POST` | `/api/host` | Start an embedded coordinator network |
| `POST` | `/api/stop_host` | Stop the embedded coordinator network |
| `POST` | `/api/routing` | Hot-update routing mode and Exit Node settings |
| `POST` | `/api/coordinator/ban` | Compatibility alias returning full client state |
| `POST` | `/api/coordinator/unban` | Compatibility alias returning full client state |

### GET /api/status

Returns a flattened status object. It intentionally uses `status`, unlike the WebSocket snapshot which uses `client_status`.

Example shape:

```json
{
  "service": "fuckipv6-unified",
  "online_clients": 1,
  "traffic": null,
  "runtime_available": true,
  "runtime_p2p_addr": "0.0.0.0:51820",
  "runtime_traffic": {
    "uptime_secs": 120,
    "tun_rx_packets": 10,
    "tun_rx_bytes": 1200,
    "tun_tx_packets": 9,
    "tun_tx_bytes": 1080,
    "p2p_rx_packets": 8,
    "p2p_rx_bytes": 960,
    "p2p_tx_packets": 9,
    "p2p_tx_bytes": 1080,
    "tun_read_errors": 0,
    "tun_write_errors": 0,
    "p2p_send_errors": 0,
    "p2p_recv_errors": 0,
    "signaling_messages_sent": 5,
    "signaling_messages_received": 4,
    "signaling_reconnects": 0
  },
  "auth_required": false,
  "status": "online",
  "virtual_ipv6": "fd00::1001",
  "virtual_ipv4": "10.8.0.2",
  "routing_mode": "MeshOnly",
  "exit_node": null,
  "is_exit_node": false,
  "connected_target": "fk://...",
  "hosting_ticket": null,
  "network_name": "default",
  "networks": [],
  "peers": [],
  "coordinator_active": false,
  "coordinator_signaling_addr": null,
  "coordinator_relay_addr": null,
  "coordinator_online_clients": 0,
  "coordinator_traffic": null,
  "coordinator_peers": [],
  "coordinator_blacklist": [],
  "logs": []
}
```

`online_clients` uses runtime peers when a client runtime is available, otherwise the embedded coordinator's online count. Coordinator fields are only meaningful while `coordinator_active` is `true`.

### GET /api/peers

Returns an array of peer snapshots. Fields include `client_id`, `virtual_ip`, optional `virtual_ipv4`, `endpoints`, `nat_type`, `connected_secs`, `idle_secs`, and optional multi-network metadata.

### POST /api/peers/probe

Runs a real P2P probe and returns `PeerProbeOutcome`.

Request:

```json
{
  "target": "<client-id-or-virtual-ip>",
  "timeout_ms": 1500
}
```

`timeout_ms` defaults to `1500`; the runtime clamps it to `100..=5000` milliseconds.

Example response:

```json
{
  "client_id": "peer-id",
  "virtual_ip": "fd00::1002",
  "endpoint": "203.0.113.10:51820",
  "rtt_us": 12400,
  "p2p_active": true,
  "timed_out": false
}
```

A probe failure returns `502` with:

```json
{"error":"..."}
```

### GET/POST /api/config

`GET` returns:

```json
{
  "mode": "DualStack",
  "ipv4_pool": "10.8.0.0/24",
  "ipv6_pool": "fd00::1000/120",
  "ipv4_prefix": 24,
  "ipv6_prefix": 120,
  "config_file": null
}
```

`POST` accepts `mode`, `ipv4_pool`, and `ipv6_pool`. It only succeeds while the embedded coordinator is running. The response is a `NetworkConfigApiResponse`:

```json
{
  "success": true,
  "message": "虚拟专网地址池参数已更新并实时生效",
  "data": {
    "mode": "DualStack",
    "ipv4_pool": "10.8.0.0/24",
    "ipv6_pool": "fd00::1000/120",
    "ipv4_prefix": 24,
    "ipv6_prefix": 120,
    "config_file": null
  }
}
```

Invalid input returns HTTP `400` with `success: false`; the HTTP status still conveys validation failure even though the response is not the generic `{ "error": ... }` shape.

### Blacklist compatibility endpoints

`GET /api/blacklist` returns an array of client IDs/fingerprints.

`POST /api/blacklist/ban` and `/api/blacklist/unban` accept:

```json
{"client_id":"peer-id"}
```

They return:

```json
{
  "success": true,
  "message": "客户端 peer-id 已封禁加入黑名单",
  "blacklist": ["peer-id"]
}
```

These compatibility handlers normally return HTTP `200` and express operation failure through `success: false`. The `/api/coordinator/ban` and `/api/coordinator/unban` aliases use the generic control-error shape and return the complete client state on success.

### POST /api/auth/login

Compatibility endpoint only. It does not authenticate access to the client WebUI. It returns `success: true`, `auth_required: false`, a placeholder token, and an expiry value.

### GET /api/logs

Query parameters:

- `service`: `coordinator`, `client`, `memory`, `errors`, a service name, or a full `.service` name. Default: `coordinator`.
- `lines`: clamped to `10..=500`. Default: `100`.

Response:

```json
{
  "service": "memory",
  "lines": ["[12:00:00] client started"],
  "error": null
}
```

### WebSocket /ws

Sends `UnifiedSnapshotResponse` JSON every two seconds. Unlike `/api/status`, the client state field is named `client_status`:

```json
{
  "service": "fuckipv6-unified",
  "online_clients": 1,
  "traffic": null,
  "runtime_available": true,
  "runtime_p2p_addr": "0.0.0.0:51820",
  "runtime_traffic": null,
  "peers": [],
  "auth_required": false,
  "client_status": "online",
  "virtual_ipv6": "fd00::1001",
  "virtual_ipv4": null,
  "routing_mode": "MeshOnly",
  "exit_node": null,
  "is_exit_node": false,
  "connected_target": "fk://...",
  "hosting_ticket": null,
  "coordinator_active": false
}
```

### Control endpoints

The following endpoints return the latest complete `AppUiState` JSON object on success:

| Endpoint | Request body |
|----------|--------------|
| `POST /api/connect` | `{"target":"fk://...","routing_mode":"MeshOnly","exit_node":null}` |
| `POST /api/disconnect` | none |
| `POST /api/host` | `{"name":"default","port":6666,"relay_port":6667,"auto_connect":true,"external_host":null}` |
| `POST /api/stop_host` | none |
| `POST /api/routing` | `{"mode":"MeshOnly","exit_node":null,"is_exit_node":false}` |
| `POST /api/coordinator/ban` | `{"client_id":"peer-id"}` |
| `POST /api/coordinator/unban` | `{"client_id":"peer-id"}` |

`AppUiState` uses `status` (for example `offline`, `connecting`, `online`, or `hosting`) and includes virtual addresses, routing settings, peers, logs, network profiles, and runtime/coordinator snapshots.

Control failures return a non-2xx status with:

```json
{"error":"连接网络失败: ..."}
```

Clients must not show a success message unless the HTTP response is successful. Two important state checks are:

- A successful `POST /api/host` response must include a non-null `hosting_ticket`.
- A successful `POST /api/stop_host` response must have `coordinator_active == false`; stale tickets must not be retained.

For `POST /api/routing`, `is_exit_node` defaults to `false` when omitted. Send it explicitly when the node's Exit Node status must be preserved.

## Client Local IPC

Default listener: `127.0.0.1:6668`.

The IPC protocol is local JSON line request/response traffic used by CLI commands such as `status` and `peers`. It is intended for local operator tooling, not remote control.
