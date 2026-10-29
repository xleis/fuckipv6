# Fuzz targets

The `fuzz` workspace is independent from the release workspace so `cargo fuzz`
can apply its sanitizer flags without forcing nightly on normal builds.

Targets:

- `wire_decoders`: data headers, cookies, rekey frames, confirmation tags and
  optional PQ wire decoders.
- `handshake_inputs`: typed HELLO/ACCEPT hashing, responder verification and
  initiator post-signature validation.
- `data_packet`: arbitrary encrypted data packets against fixed receive state.

Install the runner once:

```bash
cargo install cargo-fuzz --locked
```

Run the reproducible smoke suite from the repository root:

```bash
FUZZ_RUNS=2000 ./scripts/fuzz-smoke.sh
```

CI runs the same suite for secure-session changes, on pushes to `main`, and on
a weekly schedule. Increase `FUZZ_RUNS` or run a target without `-runs` for
longer campaigns. A crash is a release blocker; commit the minimized artifact
and a regression test before fixing the issue.
