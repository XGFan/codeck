//! Integration tests for `transcript::codex`: finding a rollout, adapting its items, and assembly
//! through `assemble` + `transcript`; plus a real-data probe.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use transcript::assemble::{LoadedFile, SessionSidecar, assemble, transcript};
use transcript::codex::{adapt, find_rollout, read};
use transcript::model::{
    AssistantBlock, FileRole, NodeBody, ToolCall, Transcript, TranscriptRequest, TranscriptScope,
};

fn transcript_of(
    id: &str,
    path: &Path,
    records: Vec<transcript::parse::EntryRecord>,
    include_hidden: bool,
) -> Transcript {
    let loaded = LoadedFile {
        path: path.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records,
    };
    let session = assemble(id, Path::new(""), vec![loaded], SessionSidecar::default());
    let req = TranscriptRequest {
        session_id: id.to_owned(),
        scope: TranscriptScope::Main,
        branch_choices: Vec::new(),
        include_hidden,
    };
    transcript(&session, &req, None).unwrap()
}

fn tool_calls(t: &Transcript) -> Vec<&ToolCall> {
    t.nodes
        .iter()
        .flat_map(|n| match &n.body {
            NodeBody::Assistant { blocks, .. } => blocks.iter().collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .filter_map(|b| match b {
            AssistantBlock::ToolCall(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn kind(n: &NodeBody) -> String {
    match n {
        NodeBody::UserPrompt { .. } => "prompt".into(),
        NodeBody::Assistant { .. } => "assistant".into(),
        NodeBody::System { level, .. } => format!("system:{}", level.as_deref().unwrap_or("")),
        NodeBody::ToolResult { .. } => "tool_result".into(),
        NodeBody::Unknown { entry_type, .. } => format!("unknown:{entry_type}"),
        other => format!("{other:?}")
            .split([' ', '{'])
            .next()
            .unwrap_or("")
            .to_owned(),
    }
}

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|v| format!("{v}\n")).collect()
}

const ID: &str = "01a21cbf-f852-73a1-8c95-c8e2c2cc9454";

fn ev(ts: &str, payload: Value) -> Value {
    json!({ "timestamp": ts, "type": "event_msg", "payload": payload })
}

fn item(ts: &str, item: Value) -> Value {
    ev(ts, json!({ "type": "item_completed", "item": item }))
}

/// A Worker's rollout: the model's view (`response_item`) is skipped, the app's items become entries.
fn rollout() -> String {
    jsonl(&[
        json!({ "timestamp": "2026-11-27T11:24:32Z", "type": "session_meta", "payload": { "id": ID, "cwd": "/repo", "originator": "haa" } }),
        json!({ "timestamp": "2026-11-27T11:24:33Z", "type": "turn_context", "payload": { "model": "gpt-6.2-sol" } }),
        json!({ "timestamp": "2026-11-27T11:24:33Z", "type": "response_item", "payload": { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Worker 规则" }] } }),
        item(
            "2026-11-27T11:24:34Z",
            json!({ "type": "UserMessage", "id": "u1", "content": [{ "type": "text", "text": "评审这份设计" }] }),
        ),
        json!({ "timestamp": "2026-11-27T11:24:35Z", "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "input": "await tools.exec_command({cmd: 'rg x'})", "call_id": "c1" } }),
        item(
            "2026-11-27T11:24:36Z",
            json!({ "type": "Reasoning", "id": "r1", "summary_text": [], "raw_content": [] }),
        ),
        item(
            "2026-11-27T11:24:37Z",
            json!({ "type": "CommandExecution", "id": "exec-1", "command": ["/bin/zsh", "-lc", "rg -n x src"], "aggregated_output": "src/a.rs:1:x\n", "exit_code": 0, "status": "completed" }),
        ),
        item(
            "2026-11-27T11:24:38Z",
            json!({ "type": "CommandExecution", "id": "exec-2", "command": ["/bin/zsh", "-lc", "cargo test"], "aggregated_output": "1 failed", "exit_code": 101, "status": "failed" }),
        ),
        item(
            "2026-11-27T11:24:39Z",
            json!({ "type": "FileChange", "id": "exec-3", "status": "completed", "changes": {
            "/repo/a.rs": { "type": "update", "unified_diff": "@@ -1,2 +1,2 @@\n fn a() {\n-    1\n+    2\n" },
            "/repo/b.rs": { "type": "add", "content": "fn b() {}\n" }
        } }),
        ),
        item(
            "2026-11-27T11:24:40Z",
            json!({ "type": "McpToolCall", "id": "call_9", "server": "haa", "tool": "report", "arguments": { "status": "done", "summary": "没问题" }, "result": { "content": [{ "type": "text", "text": "Result 已提交" }] }, "status": "completed" }),
        ),
        item(
            "2026-11-27T11:24:41Z",
            json!({ "type": "AgentMessage", "id": "m1", "content": [{ "type": "Text", "text": "已提交" }], "phase": "final_answer" }),
        ),
        item(
            "2026-11-27T11:24:42Z",
            json!({ "type": "SomethingNew", "id": "n1" }),
        ),
        ev(
            "2026-11-27T11:24:43Z",
            json!({ "type": "turn_aborted", "reason": "interrupted" }),
        ),
        ev(
            "2026-11-27T11:24:44Z",
            json!({ "type": "token_count", "info": null }),
        ),
    ])
}

#[test]
fn adapts_the_apps_items_in_order() {
    let buf = rollout();
    let (chunk, newlines) = adapt(buf.as_bytes(), ID);
    assert_eq!(
        (chunk.failed, newlines, chunk.consumed_to),
        (0, 14, buf.len() as u64)
    );
    let t = transcript_of(ID, Path::new("/x.jsonl"), chunk.records, true);
    let kinds: Vec<String> = t.nodes.iter().map(|n| kind(&n.body)).collect();
    assert_eq!(
        kinds,
        [
            "prompt",
            "assistant",
            "assistant",
            "assistant",
            "assistant",
            "assistant",
            "assistant",
            "unknown:codex:SomethingNew",
            "system:info"
        ]
    );
    let NodeBody::UserPrompt { text, .. } = &t.nodes[0].body else {
        panic!()
    };
    assert_eq!(text, "评审这份设计");
    let NodeBody::Assistant { model, blocks, .. } = &t.nodes[6].body else {
        panic!()
    };
    assert_eq!(model.as_deref(), Some("gpt-6.2-sol"));
    assert!(matches!(&blocks[0], AssistantBlock::Text { text } if text == "已提交"));

    let calls = tool_calls(&t);
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Bash", "Bash", "Edit", "Write", "mcp__haa__report"]);
    let input = |i: usize| serde_json::from_str::<Value>(&calls[i].input_json).unwrap();
    assert_eq!(input(0), json!({ "command": "rg -n x src" }));
    let ok = calls[0].result.as_ref().unwrap();
    assert!(!ok.is_error);
    assert_eq!(ok.text, "src/a.rs:1:x\n");
    let failed = calls[1].result.as_ref().unwrap();
    assert!(failed.is_error);
    assert_eq!(failed.text, "Exit code 101\n1 failed");
    assert_eq!(input(2), json!({ "file_path": "/repo/a.rs" }));
    let patch: Value = serde_json::from_str(
        calls[2]
            .result
            .as_ref()
            .unwrap()
            .extra_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        patch["structuredPatch"],
        json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "lines": [" fn a() {", "-    1", "+    2"] }])
    );
    assert_eq!(
        input(3),
        json!({ "file_path": "/repo/b.rs", "content": "fn b() {}\n" })
    );
    assert_eq!(input(4), json!({ "status": "done", "summary": "没问题" }));
    assert_eq!(calls[4].result.as_ref().unwrap().text, "Result 已提交");
}

#[test]
fn ids_are_stable_across_reads_and_appends() {
    let buf = rollout();
    let first = transcript_of(
        ID,
        Path::new("/x.jsonl"),
        adapt(buf.as_bytes(), ID).0.records,
        true,
    );
    let grown = format!(
        "{buf}{}",
        jsonl(&[item(
            "2026-11-27T11:25:00Z",
            json!({ "type": "UserMessage", "id": "u2", "content": [{ "type": "text", "text": "再看一下" }] })
        )])
    );
    let second = transcript_of(
        ID,
        Path::new("/x.jsonl"),
        adapt(grown.as_bytes(), ID).0.records,
        true,
    );
    let ids = |t: &Transcript| t.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&second)[..first.nodes.len()], ids(&first)[..]);
    assert_eq!(second.nodes.len(), first.nodes.len() + 1);
    // A partial last line waits for the next read.
    let partial = format!("{buf}{{\"type\":");
    let (chunk, _) = adapt(partial.as_bytes(), ID);
    assert_eq!((chunk.failed, chunk.consumed_to), (0, buf.len() as u64));
}

#[test]
fn finds_rollouts_plain_compressed_and_archived() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let day = root.join("sessions/2026/11/27");
    fs::create_dir_all(&day).unwrap();
    let plain = day.join(format!("rollout-2026-11-27T19-24-32-{ID}.jsonl"));
    fs::write(&plain, rollout()).unwrap();
    // A continuation part of another thread, and a thread that differs only in its last digit.
    fs::write(
        day.join(format!(
            "rollout-2026-11-27T19-30-00-{ID}_01a21cc0-0000-7000-8000-000000000000.jsonl"
        )),
        "",
    )
    .unwrap();
    assert_eq!(find_rollout(root, ID), Some(plain.clone()));
    assert_eq!(
        find_rollout(root, "01a21cbf-f852-73a1-8c95-c8e2c2cc9455"),
        None
    );
    assert_eq!(find_rollout(root, "../27"), None);
    assert_eq!(find_rollout(root, ""), None);

    let old = "019cdd9f-e4b0-7531-9911-2849cc8b2e67";
    let month = root.join("sessions/2026/03/11");
    fs::create_dir_all(&month).unwrap();
    let packed = month.join(format!("rollout-2026-03-11T23-59-29-{old}.jsonl.zst"));
    let body = rollout().replace(ID, old);
    fs::write(
        &packed,
        ruzstd::encoding::compress_to_vec(
            body.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        ),
    )
    .unwrap();
    assert_eq!(find_rollout(root, old), Some(packed.clone()));
    let (from_zst, _) = read(&packed, old).unwrap();
    assert_eq!(
        from_zst.records.len(),
        adapt(body.as_bytes(), old).0.records.len()
    );

    let archived = "01a00000-0000-7000-8000-000000000001";
    fs::create_dir_all(root.join("archived_sessions")).unwrap();
    let a = root.join(format!(
        "archived_sessions/rollout-2026-01-01T00-00-00-{archived}.jsonl"
    ));
    fs::write(&a, "").unwrap();
    assert_eq!(find_rollout(root, archived), Some(a));
}

/// `CODECK_CODEX_ROOT=~/.codex cargo test -p transcript --test codex -- --ignored --nocapture`
#[test]
#[ignore]
fn real_data_probe() {
    let root =
        std::path::PathBuf::from(std::env::var("CODECK_CODEX_ROOT").expect("CODECK_CODEX_ROOT"));
    let mut files = Vec::new();
    for y in fs::read_dir(root.join("sessions")).unwrap().flatten() {
        for m in fs::read_dir(y.path()).unwrap().flatten() {
            for d in fs::read_dir(m.path()).unwrap().flatten() {
                for f in fs::read_dir(d.path()).unwrap().flatten() {
                    files.push(f.path());
                }
            }
        }
    }
    let (mut kinds, mut tools) = (
        BTreeMap::<String, u32>::new(),
        BTreeMap::<String, u32>::new(),
    );
    let (mut failed, mut unresolved) = (0u32, 0u32);
    for p in &files {
        let name = p.file_name().unwrap().to_str().unwrap();
        let stem = name.trim_end_matches(".zst").trim_end_matches(".jsonl");
        if stem.len() > 70 && stem.as_bytes()[stem.len() - 37] == b'_' {
            // A continuation part (`…-<thread id>_<part id>`): only the thread's first file is read.
            continue;
        }
        let id = &stem[stem.len() - "01a21cbf-f852-73a1-8c95-c8e2c2cc9454".len()..];
        assert_eq!(
            find_rollout(&root, id).as_deref(),
            Some(p.as_path()),
            "{name}"
        );
        let (chunk, _) = read(p, id).unwrap();
        failed += chunk.failed;
        let t = transcript_of(id, p, chunk.records, true);
        for n in &t.nodes {
            *kinds.entry(kind(&n.body)).or_default() += 1;
        }
        for c in tool_calls(&t) {
            *tools.entry(c.name.clone()).or_default() += 1;
            if c.result.is_none() {
                unresolved += 1;
            }
        }
    }
    println!(
        "files {} failed lines {failed} calls without result {unresolved}",
        files.len()
    );
    println!("nodes {kinds:#?}");
    println!("tools {tools:#?}");
    assert_eq!(failed, 0);
    assert_eq!(unresolved, 0);
}
