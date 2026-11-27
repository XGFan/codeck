//! A haa Worker's conversation on codex or pi: not indexed, read by its Session id from the agent's
//! own file when shown (transcript, tool details), and re-read when that file grows.
use std::fs::{self, OpenOptions};
use std::io::Write;

use codeck_core::model::{
    DataRootSource, NodeBody, SessionQuery, ToolDetailRequest, ToolPart, TranscriptRequest,
    TranscriptScope,
};
use codeck_core::{CoreError, Engine, EngineConfig};
use serde_json::{Value, json};

const CODEX: &str = "01a21cbf-f852-73a1-8c95-c8e2c2cc9454";
const PI: &str = "haa-c74bd67f-2339-4238-bb70-c2bea2fcfccc";

fn line(v: Value) -> String {
    format!("{v}\n")
}

fn codex_item(ts: &str, item: Value) -> String {
    line(
        json!({ "timestamp": ts, "type": "event_msg", "payload": { "type": "item_completed", "item": item } }),
    )
}

fn req(id: &str) -> TranscriptRequest {
    TranscriptRequest {
        session_id: id.to_owned(),
        scope: TranscriptScope::Main,
        branch_choices: Vec::new(),
        include_hidden: false,
    }
}

fn prompts(engine: &Engine, id: &str) -> Vec<String> {
    engine
        .get_transcript(&req(id))
        .unwrap()
        .nodes
        .into_iter()
        .filter_map(|n| match n.body {
            NodeBody::UserPrompt { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn worker_conversations_on_codex_and_pi_are_read_by_id() {
    let claude = tempfile::tempdir().unwrap();
    fs::create_dir_all(claude.path().join("projects")).unwrap();
    let (codex, pi, cache) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );

    let day = codex.path().join("sessions/2026/11/27");
    fs::create_dir_all(&day).unwrap();
    let rollout = day.join(format!("rollout-2026-11-27T19-24-32-{CODEX}.jsonl"));
    let big = "x".repeat(10_000);
    fs::write(
        &rollout,
        [
            codex_item("2026-11-27T11:24:34Z", json!({ "type": "UserMessage", "id": "u1", "content": [{ "type": "text", "text": "评审" }] })),
            codex_item(
                "2026-11-27T11:24:35Z",
                json!({ "type": "CommandExecution", "id": "exec-1", "command": ["/bin/zsh", "-lc", "cat big"], "aggregated_output": big, "exit_code": 0, "status": "completed" }),
            ),
        ]
        .concat(),
    )
    .unwrap();
    let dir = pi.path().join("sessions/--repo--");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("2026-11-24T07-31-49-515Z_{PI}.jsonl")),
        [
            line(json!({ "type": "session", "version": 3, "id": PI, "timestamp": "2026-11-24T07:31:49Z", "cwd": "/repo" })),
            line(json!({ "type": "message", "id": "u1", "parentId": null, "timestamp": "2026-11-24T07:31:50Z", "message": { "role": "user", "content": "调研" } })),
        ]
        .concat(),
    )
    .unwrap();

    let engine = Engine::open(EngineConfig {
        data_root: claude.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: cache.path().join("__agy"),
        haa_home: cache.path().join("__haa"),
        codex_root: codex.path().to_path_buf(),
        pi_root: pi.path().to_path_buf(),
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();

    // Not Sessions of the list: only shown when asked for by id.
    assert!(
        engine
            .list_sessions(&SessionQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        engine.get_session(CODEX),
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(prompts(&engine, CODEX), ["评审"]);
    assert_eq!(prompts(&engine, PI), ["调研"]);
    assert!(matches!(
        engine.get_transcript(&req("01a21cbf-0000-7000-8000-000000000000")),
        Err(CoreError::NotFound(_))
    ));

    // The capped output comes back whole from the file.
    let detail = engine
        .get_tool_detail(&ToolDetailRequest {
            session_id: CODEX.to_owned(),
            scope: TranscriptScope::Main,
            tool_use_id: "exec-1".to_owned(),
            part: ToolPart::Output,
        })
        .unwrap();
    assert_eq!(detail.text, big);

    // A running Worker: what codex appends shows on the next read.
    let mut f = OpenOptions::new().append(true).open(&rollout).unwrap();
    f.write_all(codex_item("2026-11-27T11:25:00Z", json!({ "type": "UserMessage", "id": "u2", "content": [{ "type": "text", "text": "再看" }] })).as_bytes())
        .unwrap();
    drop(f);
    assert_eq!(prompts(&engine, CODEX), ["评审", "再看"]);
}
