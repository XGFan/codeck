#!/bin/sh
# Re-records the Connect E2E fixtures that come from ccrpc's mock-model end-to-end tests (real
# claude, local mock model, no credits): src/mocks/data/connect/{tools,turns}.jsonl, every line ccrpc
# sent and received. Each fixture's first `meta` line (which mock Session it stands for) is kept.
# The other fixtures there are kept by hand: trust.jsonl (the folder-trust dialog, the first lines
# of mock_model_session), ui-cases / session-changed / new-session / no-plugin / refused.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(mktemp -d)
(cd "$root/crates/ccrpc" && CCRPC_E2E_WIRE_DIR="$out" cargo test --test e2e mock_model_t -- --ignored --test-threads=1)
for name in tools turns; do
  f="$root/src/mocks/data/connect/$name.jsonl"
  { head -n 1 "$f" | grep '"dir": *"meta"' || true; cat "$out/mock_model_$name-0.jsonl"; } > "$f.new"
  mv "$f.new" "$f"
done
rm -rf "$out"
echo "fixtures updated: tools turns"
