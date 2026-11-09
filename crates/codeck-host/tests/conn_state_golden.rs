//! The Connect state as the Host sends it to its clients (`connect-state-event`, `connect_states`), pinned in
//! `ios/Tests/Fixtures/conn-state.json`: the iPhone app's tests decode the same file, so a change on either side
//! fails a test. `UPDATE_GOLDEN=1 cargo test -p codeck-host --test conn_state_golden` rewrites it.
//! Alone in its test binary: notice ids count from 1 per process.

use std::path::PathBuf;

use codeck_host::connect::state::ConnState;
use serde_json::{Value, json};

fn events() -> Vec<Value> {
    vec![
        json!({
            "type": "ready", "protocol": 2, "holder_id": "h1", "session_id": "s1", "cwd": "/work/app",
            "claude_version": "2.1.318", "state": "idle", "size": [120, 40], "mode": "manual",
            "model": "claude-opus-4-7", "effort": "high", "context_tokens": 51234, "warnings": [], "queue": [],
        }),
        json!({ "type": "state", "state": "busy" }),
        json!({ "type": "progress", "text": "Thinking… (12s · ↓ 1.2k tokens)", "verb": "Thinking", "elapsed_s": 12, "tokens": 1200 }),
        json!({
            "type": "permission_request", "request_id": "toolu_1", "tool_name": "Bash",
            "input": { "command": "pnpm test", "description": "Run the tests" }, "agent_id": null,
            "options": [
                { "label": "Yes", "meaning": "allow" },
                { "label": "Yes, and don't ask again for pnpm test commands in /work/app", "meaning": "allow_always" },
                { "label": "No", "meaning": "deny" },
            ],
            "layout_source": "builtin",
        }),
        json!({
            "type": "question", "request_id": "toolu_2", "agent_id": "agent-7",
            "questions": [{
                "question": "Which database?", "header": "DB", "multiSelect": false,
                "options": [{ "label": "Postgres", "description": "Relational" }, { "label": "SQLite", "preview": "file.db" }],
            }],
        }),
        json!({
            "type": "choice", "request_id": "choice-1", "title": "Ready to code?", "tool_use_id": "toolu_3",
            "layout": "plan_approval", "layout_source": "builtin",
            "options": [
                { "label": "Yes, auto-accept edits", "meaning": "allow", "description": null },
                { "label": "No, keep planning", "meaning": "deny", "description": "Tell Claude what to change" },
            ],
        }),
        json!({ "type": "state", "state": "waiting", "waiting": { "kind": "permission", "waiting_for": "Bash" } }),
        json!({ "type": "queue", "items": ["and then run lint"] }),
        json!({ "type": "session_info", "mode": "plan", "model": "claude-opus-4-7", "effort": "xhigh", "context_tokens": 52000 }),
        json!({ "type": "follow_up_failed", "text": "x", "error": { "code": "busy", "message": "还在忙" } }),
    ]
}

#[test]
fn conn_state_matches_the_ios_fixture() {
    let mut s = ConnState::new("c1", Some("h1".into()));
    for e in events() {
        s.apply(&e);
    }
    s.tasks = vec![json!({
        "id": "b1", "kind": "shell", "tool_use_id": "toolu_9", "description": "dev server",
        "command": "pnpm dev", "status": "running",
    })];
    s.last_turn = Some(json!({
        "type": "turn_end", "prompt_id": "p1", "trigger": "prompt", "last_text": "Done.", "error": null,
        "aborted": false, "restored_text": null, "background_tasks": [], "duration_ms": 8123,
        "usage": { "input_tokens": 10, "output_tokens": 20, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 0 },
        "api_cost": 0.0123,
    }));
    let got = serde_json::to_string_pretty(&s).unwrap() + "\n";

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ios/Tests/Fixtures/conn-state.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
    }
    let want =
        std::fs::read_to_string(&path).expect("run with UPDATE_GOLDEN=1 to create the fixture");
    assert_eq!(
        got, want,
        "the Connect state changed: update the iOS app, then UPDATE_GOLDEN=1"
    );
}
