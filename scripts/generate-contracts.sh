#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

UPDATE_CONTRACT=1 cargo test \
  -p fuckipv6-client --lib \
  node::contract::tests::checked_in_schema_artifacts_are_current
python3 scripts/generate_contract_bindings.py
cargo test -p fuckipv6-client --lib node::contract
python3 scripts/generate_contract_bindings.py --check
