# Node wire contracts

`contracts/schema/*.schema.json` is generated from the Rust node domain model
with JSON Schema draft 2020-12. Do not edit schema or consumer bindings by hand.

- Rust source: `crates/client/src/node`
- Schema generator: `cargo test -p fuckipv6-client --lib node::contract`
- Consumer generator: `scripts/generate_contract_bindings.py`
- Full regeneration: `scripts/generate-contracts.sh`
- Full binding/drift check: `scripts/check-contracts.sh`

Generated consumers:

- TypeScript types and a version-checking HTTP client:
  `contracts/typescript/node-contract-v1.ts`
- Android-safe Java enums and command-envelope validation:
  `contracts/android/src/main/java/cc/xlei/fuckipv6/contract/NodeContractV1.java`

## Compatibility policy

Contract version 1 accepts additive optional fields and additive enum-independent
metadata. Renaming or removing fields, changing enum meaning, making an
optional field required, or changing validation semantics requires a new
version. Every adapter must reject unsupported versions explicitly; silent
downgrade or best-effort decoding is forbidden.

`get_ui_manifest` is a bootstrap-only command. It is present in
`NodeCommandKind`, but generated UI-manifest schemas only permit the nine
user-visible action commands.

## Consumer checks

Install the pinned TypeScript dependency once, then run the complete local gate:

```bash
npm ci --ignore-scripts --prefix contracts/typescript
./scripts/check-contracts.sh
```

That gate:

- verifies Rust-generated schemas and every checked-in artifact are current;
- compiles the TypeScript binding in strict mode and compares its exported
  command/action constants against the schemas at runtime;
- exercises the TypeScript HTTP client's schema-version and request-ID
  fail-closed paths;
- compiles and runs the Java binding tests for unknown values, bootstrap-command
  rejection, contract-version rejection, and request-ID validation.
