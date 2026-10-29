# User Manual

This manual is for operators deploying and using fuckipv6. The current version has local end-to-end validation, dependency auditing, and baseline stress testing. Production deployments should still add real cross-host/cross-NAT validation and an external security review.

## How It Works

fuckipv6 creates an IPv6-only overlay network. Clients discover each other through a coordinator and receive virtual IPv6 addresses. Data is sent over UDP P2P first. If an endpoint is not verified, sending fails, or health thresholds are not met, the client falls back to the coordinator QUIC relay first and then to the WebSocket signaling relay if QUIC relay is unavailable.

## Installation

Build from source:

```bash
cargo build --release
```

Binaries:

- `target/release/fuckipv6`
- `target/release/fuckipv6-coordinator`

Linux clients need root or `CAP_NET_ADMIN` to create a TUN device. macOS clients need sudo/root to create utun and configure IPv6 routes; the default `tun.name = "fuckipv6"` lets the system allocate the actual `utunN` interface. Windows clients need administrator privileges and WinTun.

## Start The Coordinator

```bash
./target/release/fuckipv6-coordinator
```

Default ports:

- TCP `6666`: WebSocket signaling.
- TCP `8080`: Web dashboard and HTTP API.

Custom bind addresses:

```bash
./target/release/fuckipv6-coordinator --bind 0.0.0.0:6666 --webui-bind 0.0.0.0:8080
```

Web UI:

```text
http://<coordinator>:8080
```

## Manage Client Network Profiles

Save a network:

```bash
fuckipv6 join example.com:6666 --name office
```

List saved networks:

```bash
fuckipv6 list
```

Set the default network:

```bash
fuckipv6 default office
```

Connect to the default network:

```bash
sudo fuckipv6 connect
```

Connect to a named network:

```bash
sudo fuckipv6 connect office
```

Connect temporarily without saving config:

```bash
sudo fuckipv6 connect example.com:6666 --temp
```

Delete a saved network:

```bash
fuckipv6 leave office
```

## Start With A Config File

Generate config:

```bash
fuckipv6 init-config client.toml
```

Edit `coordinator_url`:

```toml
coordinator_url = "ws://example.com:6666"
```

Start:

```bash
sudo fuckipv6 start --config client.toml
```

## Daily Operations

Show runtime status:

```bash
sudo fuckipv6 status
```

Show peers:

```bash
sudo fuckipv6 peers
```

Test virtual IPv6 connectivity:

```bash
ping6 fd00::1002
```

Or:

```bash
fuckipv6 ping fd00::1002 --count 4
```

Show routes:

```bash
fuckipv6 route
```

Show logs:

```bash
fuckipv6 logs --service client --lines 100
fuckipv6 logs --service coordinator --lines 100
```

Show TUN device statistics:

```bash
fuckipv6 stats
```

## Configuration

Common client config:

```toml
coordinator_url = "ws://example.com:6666"
private_key_path = "identity.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6"
mtu = 1280

[p2p]
listen_port = 0
connect_timeout = 10
heartbeat_interval = 30

[p2p.fallback_threshold]
max_rtt_ms = 200
max_loss_rate = 0.05
timeout_secs = 30

[log]
level = "info"
```

`RUST_LOG` overrides the configured log level.

While running, the client watches the config file and hot-reloads `[p2p.fallback_threshold]`. Changes to `log.level` are detected and logged, but they require a client restart to take effect.

## Encrypted Config Files

Use `FUCKIPV6_CONFIG_PASSPHRASE` to provide the passphrase. The encrypted config format uses Argon2id key derivation and ChaCha20-Poly1305 authenticated encryption.

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 encrypt-config client.toml client.toml.enc
```

Start with an encrypted config:

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' sudo -E fuckipv6 start --config client.toml.enc
```

Decrypt to a plaintext file:

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 decrypt-config client.toml.enc --output client.toml
```

## Troubleshooting

Permission errors:

```text
failed to create TUN device
```

Run Linux/macOS clients with `sudo`; on Linux you may also grant `CAP_NET_ADMIN` to the binary. Run Windows clients from an administrator terminal.

Leftover TUN device:

```bash
sudo ip link delete fuckipv6
```

On macOS, utun is released when the process exits. If an IPv6 route or address remains, inspect it with `netstat -rn -f inet6` and `ifconfig utunN`, then delete the stale route or address.

Cannot connect to the coordinator:

- Ensure `coordinator_url` uses `ws://` or `wss://`.
- Ensure TCP 6666 is open on the firewall.
- Ensure the client can reach the coordinator public address.

P2P is not direct, but virtual IP traffic works:

- Relay fallback is active.
- Use `fuckipv6 peers` to inspect RTT, loss, probe success, and active endpoint.
- Use the Web UI to watch relay payload counters.

## Current Limits

- NAT type detection depends on reachable STUN/RFC5780 servers; insufficient evidence is reported as Unknown.
- The direct P2P data plane currently uses UDP; QUIC is implemented for relay fallback, certificate pinning, multiplexing, and 0-RTT self-tests.
- QUIC relay supports registration, forwarding, rate limiting, concurrency limits, and loopback integration tests.
- Single-host Linux network namespace testing now covers three nodes, routes, mutual ping, and near-MTU ping. Windows and macOS builds are covered by CI. Production deployments should still add real cross-host/cross-NAT validation and larger stress testing.
