#!/bin/sh
# A sandbox for trying cc-gui for real without touching your own ToH / claude / ccrpc: its own
# TOH_STATE_DIR, CLAUDE_CONFIG_DIR and CCRPC_STATE_DIR under /tmp/ccgui-sandbox, Claude Code talking
# to scripts/sandbox/mock_model.py on loopback (start it with `python3 -I mock_model.py 18765`), the
# ccrpc Plugin loaded with --plugin-dir (a wrapper adds it to `ccrpc new`). Prints the work folder.
# Used by `cargo test --test sandbox -- --ignored` and by scripts/sandbox/run-app.sh.
set -eu
B=${CC_GUI_SANDBOX:-/tmp/ccgui-sandbox}; PORT=${PORT:-18765}
REAL=${CCRPC_BIN:-$(cd "$(dirname "$0")/../../../.." && pwd)/target/release/ccrpc}
rm -rf "$B"; mkdir -p "$B/toh" "$B/cc" "$B/st" "$B/work" "$B/layouts"
chmod 700 "$B/toh" "$B/st"
W=$(cd "$B/work" && pwd -P)
cat > "$B/toh/config.toml" <<C
language = "zh-Hans"
[backends]
enabled = ["clay"]
[lan]
bind = "127.0.0.1"
mdns = false
[backends.clay]
env = { CLAUDE_CONFIG_DIR = "$B/cc", CCRPC_STATE_DIR = "$B/st", ANTHROPIC_BASE_URL = "http://127.0.0.1:$PORT", ANTHROPIC_AUTH_TOKEN = "mock", ANTHROPIC_MODEL = "claude-sonnet-4-5", ANTHROPIC_DEFAULT_HAIKU_MODEL = "claude-sonnet-4-5", DISABLE_AUTOUPDATER = "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = "1" }
C
chmod 600 "$B/toh/config.toml"
printf '{"hasCompletedOnboarding":true,"projects":{"%s":{"hasTrustDialogAccepted":true}}}' "$W" > "$B/cc/.claude.json"
printf '{"permissions":{"defaultMode":"default"}}' > "$B/cc/settings.json"
echo hello > "$B/work/a.txt"
"$REAL" plugin generate "$B/plugin" >/dev/null
cat > "$B/ccrpc-wrap" <<C
#!/bin/sh
if [ "\$1" = new ]; then exec "$REAL" "\$@" -- --plugin-dir "$B/plugin/ccrpc"; else exec "$REAL" "\$@"; fi
C
chmod +x "$B/ccrpc-wrap"
echo "$W"
