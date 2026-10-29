# Contributing

Thanks for working on fuckipv6. This project is still pre-1.0, so changes should be small, testable, and explicit about what is production-ready versus planned.

## Development Setup

Requirements:

- Rust stable
- Linux for full TUN runtime testing, or Windows with administrator access and WinTun for Windows client work
- `iproute2` on Linux when testing TUN setup

Build and test:

```bash
cargo fmt --check
cargo test --workspace
cargo build --release
```

Run clippy when touching shared logic or before a release:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## Code Guidelines

- Prefer existing module boundaries and helper APIs.
- Keep protocol changes backward-compatible where possible and document them in `API.md`.
- Add focused tests for behavior changes.
- Avoid marking TODO items complete unless the current code and verification prove the item.
- Use `anyhow::Context` for operator-facing errors where the failing operation is not obvious.
- Keep normal `info` logs low-volume. Packet-level and message-level logs belong at `debug` or `trace`.

## Testing Expectations

For most changes, run:

```bash
cargo fmt --check
cargo test --workspace
cargo build --release
```

For networking changes, also test the relevant runtime path:

- Coordinator registration and WebSocket reconnect.
- Two-client P2P data forwarding.
- Relay fallback when P2P is unavailable.
- TUN setup and cleanup.
- Windows changes on an actual Windows host.

## Documentation

Update documentation with code changes:

- `README.md`: user-facing feature summary and quick links.
- `ARCHITECTURE.md`: module boundaries and data flow.
- `API.md`: signaling, P2P, HTTP, and IPC protocol changes.
- `CHANGELOG.md`: user-visible changes.
- `TODO.md`: only update checkboxes when implementation and verification are complete.

## Security

Do not commit private keys, generated identities, service secrets, logs with public IPs unless sanitized, or release artifacts containing local configuration. Authentication currently relies on Ed25519 identities and public-key fingerprint client IDs; treat changes to `crypto.rs`, registration, relay, or config storage as security-sensitive.
