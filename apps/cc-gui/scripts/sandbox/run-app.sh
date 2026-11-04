#!/bin/sh
# Runs the built cc-gui inside the sandbox (scripts/sandbox/env.sh) with the mock model.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
B=${CC_GUI_SANDBOX:-/tmp/ccgui-sandbox}; PORT=${PORT:-18765}
python3 -I "$here/mock_model.py" "$PORT" & mock=$!
trap 'kill $mock 2>/dev/null' EXIT
"$here/env.sh" >/dev/null
toh_dir=$(cd "$here/../../../toh/target/debug" 2>/dev/null && pwd || true)
app="$here/../../src-tauri/target/release/bundle/macos/cc-gui.app/Contents/MacOS/cc-gui"
env -u TOH_CONFIG -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u TOH_HOLDER_ID \
  TOH_STATE_DIR="$B/toh" CLAUDE_CONFIG_DIR="$B/cc" CCRPC_STATE_DIR="$B/st" CCRPC_LAYOUT_DIR="$B/layouts" \
  CC_GUI_CCRPC="$B/ccrpc-wrap" PATH="${toh_dir:+$toh_dir:}$PATH" "$app"
