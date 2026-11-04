#!/bin/sh
# Re-records the drive E2E fixture src/mocks/data/drive/tools.jsonl from ccrpc's mock-model
# end-to-end test (real claude, local mock model, no credits): every line ccrpc sent and received.
# The fixture's first `meta` line (which mock Session it stands for) is kept.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(mktemp -d)
(cd "$root/crates/ccrpc" && CCRPC_E2E_WIRE_DIR="$out" cargo test --test e2e mock_model_tools -- --ignored --test-threads=1)
f="$root/src/mocks/data/drive/tools.jsonl"
{ head -n 1 "$f" | grep '"dir": *"meta"' || true; cat "$out/mock_model_tools-0.jsonl"; } > "$f.new"
mv "$f.new" "$f"
rm -rf "$out"
echo "fixture updated: src/mocks/data/drive/tools.jsonl"
