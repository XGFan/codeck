#!/bin/sh
# Runs the built Claude Viewer inside the sandbox (scripts/drive-sandbox/env.sh) with the mock model.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
B=${CV_SANDBOX:-/tmp/cv-drive-sandbox}; PORT=${PORT:-18765}
python3 -I "$here/mock_model.py" "$PORT" & mock=$!
trap 'kill $mock 2>/dev/null' EXIT
"$here/env.sh" >/dev/null
toh_dir=$(cd "$root/../toh/target/debug" 2>/dev/null && pwd || true)
app="$root/target/release/bundle/macos/Claude Viewer.app/Contents/MacOS/claude-viewer"
env -u TOH_CONFIG -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u TOH_HOLDER_ID \
  TOH_STATE_DIR="$B/toh" CLAUDE_CONFIG_DIR="$B/cc" CCRPC_STATE_DIR="$B/st" CCRPC_LAYOUT_DIR="$B/layouts" \
  CV_CCRPC="$B/ccrpc-wrap" PATH="${toh_dir:+$toh_dir:}$PATH" "$app"
