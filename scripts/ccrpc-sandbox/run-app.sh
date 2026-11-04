#!/bin/sh
# Runs the built CoDeck inside the sandbox (scripts/ccrpc-sandbox/env.sh) with the mock model.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
B=${CODECK_SANDBOX:-/tmp/codeck-ccrpc-sandbox}; PORT=${PORT:-18765}
python3 -I "$here/mock_model.py" "$PORT" & mock=$!
trap 'kill $mock 2>/dev/null' EXIT
"$here/env.sh" >/dev/null
toh_dir=$(cd "$root/../toh/target/debug" 2>/dev/null && pwd || true)
app="$root/target/release/bundle/macos/CoDeck.app/Contents/MacOS/codeck"
env -u TOH_CONFIG -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u TOH_HOLDER_ID \
  TOH_STATE_DIR="$B/toh" CLAUDE_CONFIG_DIR="$B/cc" CCRPC_STATE_DIR="$B/st" CCRPC_LAYOUT_DIR="$B/layouts" \
  CODECK_CCRPC="$B/ccrpc-wrap" PATH="${toh_dir:+$toh_dir:}$PATH" "$app"
