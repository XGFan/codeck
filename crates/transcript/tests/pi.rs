//! Integration tests for `transcript::pi`: finding a session file, adapting its entries on pi's
//! tree, and assembly through `assemble` + `transcript`; plus a real-data probe.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use transcript::assemble::{LoadedFile, SessionSidecar, assemble, transcript};
use transcript::model::{
    AssistantBlock, FileRole, NodeBody, PromptOrigin, ToolCall, Transcript, TranscriptRequest,
    TranscriptScope,
};
use transcript::pi::{adapt, find_session, read};

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

const ID: &str = "haa-c74bd67f-2339-4238-bb70-c2bea2fcfccc";

fn msg(id: &str, parent: Option<&str>, ts: &str, message: Value) -> Value {
    json!({ "type": "message", "id": id, "parentId": parent, "timestamp": ts, "message": message })
}

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({ "type": "toolCall", "id": id, "name": name, "arguments": arguments })
}

fn result(call: &str, name: &str, text: &str, is_error: bool) -> Value {
    json!({ "role": "toolResult", "toolCallId": call, "toolName": name, "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// A Worker's session: settings entries between messages, every built-in tool, an error reply.
fn session() -> String {
    jsonl(&[
        json!({ "type": "session", "version": 3, "id": ID, "timestamp": "2026-11-24T07:31:49.515Z", "cwd": "/repo" }),
        json!({ "type": "model_change", "id": "mc", "parentId": null, "timestamp": "2026-11-24T07:31:49.567Z", "provider": "agent-gateway", "modelId": "deepseek-v4.1-flash" }),
        json!({ "type": "thinking_level_change", "id": "tl", "parentId": "mc", "thinkingLevel": "off" }),
        msg(
            "sys",
            Some("tl"),
            "2026-11-24T07:31:49.576Z",
            json!({ "role": "system", "content": "你是 Worker" }),
        ),
        msg(
            "u1",
            Some("sys"),
            "2026-11-24T07:31:50Z",
            json!({ "role": "user", "content": [{ "type": "text", "text": "改一下 a.rs" }] }),
        ),
        msg(
            "a1",
            Some("u1"),
            "2026-11-24T07:31:51Z",
            json!({
                "role": "assistant", "model": "deepseek-v4.1-flash",
                "usage": { "input": 100, "output": 20, "cacheRead": 5, "cacheWrite": 0 },
                "content": [
                    { "type": "thinking", "thinking": "先看文件" },
                    call("c1", "bash", json!({ "command": "ls", "timeout": 10 })),
                    call("c2", "read", json!({ "path": "/repo/a.rs", "offset": 1, "limit": 20 })),
                    call("c3", "edit", json!({ "path": "/repo/a.rs", "edits": [{ "oldText": "1", "newText": "2" }] })),
                    call("c4", "edit", json!({ "path": "/repo/b.rs", "edits": [{ "oldText": "a", "newText": "b" }, { "oldText": "c", "newText": "d" }] })),
                    call("c5", "write", json!({ "path": "/repo/c.rs", "content": "fn c() {}" })),
                    call("c6", "grep", json!({ "pattern": "fn", "path": "/repo", "ignoreCase": true, "context": 2 })),
                    call("c7", "find", json!({ "pattern": "*.rs" })),
                    call("c8", "ls", json!({ "path": "/repo" })),
                    call("c9", "report", json!({ "status": "done", "summary": "改好了" }))
                ]
            }),
        ),
        msg(
            "r1",
            Some("a1"),
            "2026-11-24T07:31:52Z",
            result("c1", "bash", "a.rs", false),
        ),
        json!({ "type": "custom", "id": "cu", "parentId": "r1", "customType": "haa.result", "data": {} }),
        msg(
            "r2",
            Some("cu"),
            "2026-11-24T07:31:53Z",
            result("c2", "read", "fn a() {}", false),
        ),
        msg(
            "r3",
            Some("r2"),
            "2026-11-24T07:31:54Z",
            result("c3", "edit", "No match", true),
        ),
        msg(
            "a2",
            Some("r3"),
            "2026-11-24T07:31:55Z",
            json!({ "role": "assistant", "content": [], "stopReason": "error", "errorMessage": "Connection error." }),
        ),
        msg(
            "b1",
            Some("a2"),
            "2026-11-24T07:31:56Z",
            json!({ "role": "bashExecution", "command": "git status", "output": "clean\n", "exitCode": 0 }),
        ),
        json!({ "type": "something_new", "id": "sn", "parentId": "b1" }),
    ])
}

#[test]
fn adapts_messages_and_translates_tools() {
    let buf = session();
    let (chunk, newlines) = adapt(buf.as_bytes(), ID);
    assert_eq!((chunk.failed, newlines), (0, 13));
    let t = transcript_of(ID, Path::new("/x.jsonl"), chunk.records, true);
    let kinds: Vec<String> = t.nodes.iter().map(|n| kind(&n.body)).collect();
    // A `!` command shows as Claude Code's does: the command, then its output.
    assert_eq!(
        kinds,
        [
            "system:info",
            "prompt",
            "assistant",
            "system:error",
            "prompt",
            "prompt",
            "unknown:pi:something_new"
        ]
    );
    let NodeBody::UserPrompt { text, origin, .. } = &t.nodes[4].body else {
        panic!()
    };
    assert_eq!(text, "<bash-input>git status</bash-input>");
    assert!(!matches!(origin, PromptOrigin::Human));
    let NodeBody::Assistant {
        model,
        blocks,
        usage,
        ..
    } = &t.nodes[2].body
    else {
        panic!()
    };
    assert_eq!(model.as_deref(), Some("deepseek-v4.1-flash"));
    assert!(matches!(&blocks[0], AssistantBlock::Thinking { text } if text == "先看文件"));
    assert_eq!(
        usage.as_ref().map(|u| (u.input, u.output)),
        Some((100.0, 20.0))
    );

    let calls = tool_calls(&t);
    let shown: Vec<(&str, Value)> = calls
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                serde_json::from_str(&c.input_json).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("Bash", json!({ "command": "ls", "timeout": 10 })),
            (
                "Read",
                json!({ "file_path": "/repo/a.rs", "offset": 1, "limit": 20 })
            ),
            (
                "Edit",
                json!({ "file_path": "/repo/a.rs", "old_string": "1", "new_string": "2" })
            ),
            (
                "MultiEdit",
                json!({ "file_path": "/repo/b.rs", "edits": [{ "old_string": "a", "new_string": "b" }, { "old_string": "c", "new_string": "d" }] })
            ),
            (
                "Write",
                json!({ "file_path": "/repo/c.rs", "content": "fn c() {}" })
            ),
            (
                "Grep",
                json!({ "pattern": "fn", "path": "/repo", "-i": true, "-C": 2 })
            ),
            ("Glob", json!({ "pattern": "*.rs" })),
            ("LS", json!({ "path": "/repo" })),
            ("report", json!({ "status": "done", "summary": "改好了" })),
        ]
    );
    // Results reach their calls across a skipped `custom` entry.
    let texts: Vec<Option<(&str, bool)>> = calls[..4]
        .iter()
        .map(|c| c.result.as_ref().map(|r| (r.text.as_str(), r.is_error)))
        .collect();
    assert_eq!(
        texts,
        [
            Some(("a.rs", false)),
            Some(("fn a() {}", false)),
            Some(("No match", true)),
            None
        ]
    );
}

#[test]
fn follows_pi_tree_branches() {
    // u2 answers a1 again: a Branch, the later reply being on the Main Line.
    let buf = jsonl(&[
        json!({ "type": "session", "version": 3, "id": ID, "timestamp": "2026-11-24T07:00:00Z", "cwd": "/repo" }),
        msg(
            "u1",
            None,
            "2026-11-24T07:00:01Z",
            json!({ "role": "user", "content": "第一问" }),
        ),
        msg(
            "a1",
            Some("u1"),
            "2026-11-24T07:00:02Z",
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "答一" }] }),
        ),
        msg(
            "u2",
            Some("a1"),
            "2026-11-24T07:00:03Z",
            json!({ "role": "user", "content": "追问甲" }),
        ),
        json!({ "type": "model_change", "id": "mc", "parentId": "a1", "timestamp": "2026-11-24T07:00:04Z", "provider": "p", "modelId": "m" }),
        msg(
            "u3",
            Some("mc"),
            "2026-11-24T07:00:05Z",
            json!({ "role": "user", "content": "追问乙" }),
        ),
    ]);
    let t = transcript_of(
        ID,
        Path::new("/x.jsonl"),
        adapt(buf.as_bytes(), ID).0.records,
        false,
    );
    let prompts: Vec<&str> = t
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::UserPrompt { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, ["第一问", "追问乙"]);
    assert_eq!(t.branch_points.len(), 1);
}

#[test]
fn finds_session_files_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cwd = root.join("sessions/--Users-me-repo--");
    fs::create_dir_all(&cwd).unwrap();
    let f = cwd.join(format!("2026-11-24T07-31-49-515Z_{ID}.jsonl"));
    fs::write(&f, session()).unwrap();
    fs::write(cwd.join("2026-11-24T07-31-49-515Z_haa-other.jsonl"), "").unwrap();
    assert_eq!(find_session(root, ID), Some(f.clone()));
    assert_eq!(
        find_session(root, "c74bd67f-2339-4238-bb70-c2bea2fcfccc"),
        None
    );
    assert_eq!(find_session(root, "../x"), None);
    let (chunk, _) = read(&f, ID).unwrap();
    assert_eq!(
        chunk.records.len(),
        adapt(session().as_bytes(), ID).0.records.len()
    );
}

/// `CODECK_PI_ROOT=~/.pi/agent cargo test -p transcript --test pi -- --ignored --nocapture`
#[test]
#[ignore]
fn real_data_probe() {
    let root = std::path::PathBuf::from(std::env::var("CODECK_PI_ROOT").expect("CODECK_PI_ROOT"));
    let mut files = Vec::new();
    for d in fs::read_dir(root.join("sessions")).unwrap().flatten() {
        for f in fs::read_dir(d.path()).unwrap().flatten() {
            files.push(f.path());
        }
    }
    let (mut kinds, mut tools) = (
        BTreeMap::<String, u32>::new(),
        BTreeMap::<String, u32>::new(),
    );
    let (mut failed, mut unresolved, mut branched) = (0u32, 0u32, 0u32);
    for p in &files {
        let name = p.file_name().unwrap().to_str().unwrap();
        let Some((_, rest)) = name.split_once('_') else {
            continue;
        };
        let id = rest.trim_end_matches(".jsonl");
        assert_eq!(
            find_session(&root, id).as_deref(),
            Some(p.as_path()),
            "{name}"
        );
        let (chunk, _) = read(p, id).unwrap();
        failed += chunk.failed;
        let t = transcript_of(id, p, chunk.records, true);
        if !t.branch_points.is_empty() {
            branched += 1;
        }
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
        "files {} failed lines {failed} calls without result {unresolved} with branches {branched}",
        files.len()
    );
    println!("nodes {kinds:#?}");
    println!("tools {tools:#?}");
    assert_eq!(failed, 0);
}
