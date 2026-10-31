#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo test -p fuckipv6-client --lib node::contract
python3 scripts/generate_contract_bindings.py --check
npm run check --prefix contracts/typescript
npm run build:test --prefix contracts/typescript
node scripts/test_contract_bindings.mjs
node scripts/test_webui_node_ui.mjs

rm -rf target/contract-java
mkdir -p target/contract-java
javac --release 8 -d target/contract-java \
  contracts/android/src/main/java/cc/xlei/fuckipv6/contract/NodeContractV1.java \
  contracts/android/src/test/java/cc/xlei/fuckipv6/contract/NodeContractV1Test.java
java -ea -cp target/contract-java cc.xlei.fuckipv6.contract.NodeContractV1Test
