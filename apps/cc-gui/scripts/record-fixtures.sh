#!/bin/sh
# Records E2E fixtures from ccrpc's mock-model end-to-end tests (real claude, local mock model,
# no credits): every ccrpc's stdin / stdout, as cc-gui's wire.jsonl.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
ccrpc=${CCRPC_DIR:-$here/../ccrpc}
out=$(mktemp -d)
(cd "$ccrpc" && CCRPC_E2E_WIRE_DIR="$out" cargo test --test e2e mock_ -- --ignored --test-threads=1)
for name in tools session turns; do
  cp "$out/mock_model_$name-0.jsonl" "$here/e2e/fixtures/$name.jsonl"
done
rm -rf "$out"
echo "fixtures updated: tools session turns"
