//! Integration tests for `transcript::agy`: path layout, parsing, adaptation, full assembly
//! through `assemble::assemble` + `assemble::transcript`, metadata decoding, and a real-data probe.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use transcript::agy::{
    self, UnknownKind, adapt, brain_dir, is_haa_workspace, metadata_path, parse, parse_metadata,
    presence_dir, read, read_metadata, session_of_presence, session_of_transcript, transcript_path,
};
use transcript::assemble::{
    AssembledSession, LoadedFile, SessionSidecar, assemble, summarize, transcript,
};
use transcript::model::{
    AssistantBlock, FileRole, NodeBody, PromptOrigin, ToolCall, Transcript, TranscriptRequest,
    TranscriptScope,
};

fn main_t(s: &AssembledSession, include_hidden: bool) -> Transcript {
    let req = TranscriptRequest {
        session_id: s.session_id.clone(),
        scope: TranscriptScope::Main,
        branch_choices: Vec::new(),
        include_hidden,
    };
    transcript(s, &req, None).unwrap()
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

fn jsonl(lines: &[Value]) -> String {
    let mut s = String::new();
    for v in lines {
        s.push_str(&v.to_string());
        s.push('\n');
    }
    s
}

fn assemble_agy(
    conv_id: &str,
    path: &Path,
) -> (
    agy::AgyFile,
    agy::Adapted,
    transcript::assemble::AssembledSession,
) {
    let parsed = read(path).expect("read agy file");
    let adapted = adapt(conv_id, &parsed);
    // Re-adapt so the caller also keeps `adapted` for assertions while `assemble` owns `records`.
    let for_assemble = adapt(conv_id, &parsed);
    let loaded = LoadedFile {
        path: path.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: for_assemble.records,
    };
    let session_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let session = assemble(
        conv_id,
        session_dir,
        vec![loaded],
        SessionSidecar::default(),
    );
    (parsed, adapted, session)
}

#[test]
fn layout_paths_and_session_extraction() {
    let root = Path::new("/data/agy");
    assert_eq!(brain_dir(root), PathBuf::from("/data/agy/brain"));
    assert_eq!(
        metadata_path(root),
        PathBuf::from("/data/agy/cache/conversation_metadata.json")
    );
    assert_eq!(presence_dir(root), PathBuf::from("/data/agy/presence"));

    let tp = transcript_path(root, "conv-123");
    assert_eq!(
        tp,
        PathBuf::from("/data/agy/brain/conv-123/.system_generated/logs/transcript_full.jsonl")
    );
    assert_eq!(
        session_of_transcript(root, &tp).as_deref(),
        Some("conv-123")
    );
    assert_eq!(
        session_of_transcript(root, Path::new("/data/agy/brain/conv-123/other.jsonl")),
        None
    );
    assert_eq!(
        session_of_transcript(
            root,
            Path::new("/data/agy/brain/a/b/.system_generated/logs/transcript_full.jsonl")
        ),
        None
    );
    assert_eq!(
        session_of_transcript(
            root,
            Path::new("/other/brain/conv-123/.system_generated/logs/transcript_full.jsonl")
        ),
        None
    );

    let lock = presence_dir(root).join("conv-123.lock");
    assert_eq!(
        session_of_presence(root, &lock).as_deref(),
        Some("conv-123")
    );
    assert_eq!(
        session_of_presence(root, &presence_dir(root).join(".lock")),
        None
    );
    assert_eq!(
        session_of_presence(root, &presence_dir(root).join("sub/conv-123.lock")),
        None
    );
    assert_eq!(
        session_of_presence(root, &presence_dir(root).join("conv-123.txt")),
        None
    );
}

#[test]
fn parse_handles_bad_lines_dedup_sorting_and_partial_tail() {
    let l1 = json!({"step_index": 2, "type": "USER_INPUT", "content": "second"}).to_string();
    let l2 = "[1, 2, 3]"; // not an object -> failed
    let l3 = json!({"type": "USER_INPUT", "content": "no index"}).to_string(); // missing step_index -> failed
    let l4 = "{broken json"; // syntax error -> failed
    let l5 = json!({"step_index": 0, "type": "USER_INPUT", "content": "first"}).to_string();
    let l6 =
        json!({"step_index": 2, "type": "USER_INPUT", "content": "second-updated"}).to_string();
    let partial = r#"{"step_index":3,"type":"USER_INPUT","content":"unfinished"#;

    let body = format!("{l1}\n\n{l2}\n{l3}\n{l4}\n{l5}\n{l6}\n{partial}");
    let expected_consumed = (body.len() - partial.len()) as u64;
    let f = parse(body.as_bytes());

    assert_eq!(f.lines, 7);
    assert_eq!(f.consumed_to, expected_consumed);
    assert_eq!(f.failed, 3);
    assert_eq!(
        f.first_error.as_deref(),
        Some(format!("字节偏移 {}：不是 JSON 对象", l1.len() + 2).as_str())
    );
    assert_eq!(f.steps.len(), 2);
    assert_eq!(f.steps[0].index, 0);
    assert_eq!(f.steps[0].content.as_deref(), Some("first"));
    assert_eq!(f.steps[0].line_no, 6);
    assert_eq!(f.steps[1].index, 2);
    assert_eq!(f.steps[1].content.as_deref(), Some("second-updated"));
    assert_eq!(f.steps[1].line_no, 7);
    let l6_offset = body.find(&l6).unwrap() as u64;
    assert_eq!(f.steps[1].offset, l6_offset);
}

#[test]
fn adapts_all_step_types_tools_and_assembles_into_transcript_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript_full.jsonl");
    let conv_id = "conv-full-test";

    let lines = vec![
        // 0: USER_INPUT with <USER_REQUEST>, nested </USER_REQUEST> mention, <ADDITIONAL_METADATA>, <USER_SETTINGS_CHANGE>
        json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:00Z",
            "content": "<USER_REQUEST>\nFix the bug. Note: keep `</USER_REQUEST>` inside backticks safe.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-11-27T10:00:00+08:00.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection`.\n</USER_SETTINGS_CHANGE>"
        }),
        // 1: PLANNER_RESPONSE with thinking, text, tokens, and all 6 translated tools + fallback + known untranslated + mcp_haa + unknown
        json!({
            "step_index": 1,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:01Z",
            "model": "gemini-4-neon-high",
            "input_tokens": 1000,
            "cache_read_tokens": 250,
            "output_tokens": 80,
            "thinking": "Let me inspect and update the files.",
            "content": "Running checks now.",
            "tool_calls": [
                {"name": "run_command", "args": {"CommandLine": "cargo test", "WaitMsBeforeAsync": 5000, "toolSummary": "Run tests"}},
                {"name": "read_file", "args": {"Path": "/repo/src/lib.rs", "StartLine": 10, "EndLine": 30}},
                {"name": "edit_file", "args": {"Path": "/repo/src/lib.rs", "Target": "old", "Replacement": "new"}},
                {"name": "write_file", "args": {"Path": "/repo/out.txt", "Content": "hello", "Overwrite": true}},
                {"name": "update_steps", "args": {"steps": [
                    {"title": "Step A", "status": "done"},
                    {"title": "Step B", "status": "in_progress"},
                    {"title": "Step C", "status": "skipped"},
                    {"title": "Step D", "status": "pending"}
                ]}},
                {"name": "search_web", "args": {"query": "rust serde rawvalue"}},
                // Missing required param -> stays run_command without agyOriginal, still known
                {"name": "run_command", "args": {"toolSummary": "missing command"}},
                // Known untranslated tools
                {"name": "manage_task", "args": {"Action": "list"}},
                {"name": "mcp_haa_report", "args": {"status": "done", "summary": "ok"}},
                // Unknown tool
                {"name": "mystery_tool", "args": {"x": 1}}
            ]
        }),
        // 2: GENERIC for run_command (double Created At / Completed At headers)
        json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "GENERIC",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:02Z",
            "content": "Created At: 2026-11-27T10:00:02+08:00\nCompleted At: 2026-11-27T10:00:03+08:00\nCreated At: 2026-11-27T10:00:02+08:00\nCompleted At: 2026-11-27T10:00:03+08:00\nCommand exited with code 0.\n\nCommand output:\nall passed\n"
        }),
        // 3: GENERIC for read_file (RUNNING background task note)
        json!({
            "step_index": 3,
            "source": "MODEL",
            "type": "GENERIC",
            "status": "RUNNING",
            "created_at": "2026-11-27T02:00:03Z",
            "content": "Created At: 2026-11-27T10:00:03+08:00\nTool is running as a background task with task id: conv/task-3"
        }),
        // 4: GENERIC for edit_file (ERROR)
        json!({
            "step_index": 4,
            "source": "MODEL",
            "type": "GENERIC",
            "status": "ERROR",
            "error": "target not found",
            "created_at": "2026-11-27T02:00:04Z",
            "content": "Created At: 2026-11-27T10:00:04+08:00\nCompleted At: 2026-11-27T10:00:04+08:00\nEncountered error: target not found"
        }),
        // 5..=11: remaining 7 tool results
        json!({"step_index": 5, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:05Z", "content": "Created At: t\nCompleted At: t\nCreated file."}),
        json!({"step_index": 6, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:06Z", "content": "Created At: t\nCompleted At: t\nSteps updated."}),
        json!({"step_index": 7, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:07Z", "content": "Created At: t\nCompleted At: t\nSearch results."}),
        json!({"step_index": 8, "type": "GENERIC", "status": "ERROR", "created_at": "2026-11-27T02:00:08Z", "error": "missing CommandLine"}),
        json!({"step_index": 9, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:09Z", "content": "No tasks."}),
        json!({"step_index": 10, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:10Z", "content": "Reported."}),
        json!({"step_index": 11, "type": "GENERIC", "status": "DONE", "created_at": "2026-11-27T02:00:11Z", "content": "Mystery ok."}),
        // 12: SYSTEM_MESSAGE with preamble containing <SYSTEM_MESSAGE>
        json!({
            "step_index": 12,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:12Z",
            "content": "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system.\n\n<SYSTEM_MESSAGE>\nBackground task finished.\n</SYSTEM_MESSAGE>"
        }),
        // 13: ERROR_MESSAGE
        json!({
            "step_index": 13,
            "source": "SYSTEM",
            "type": "ERROR_MESSAGE",
            "status": "ERROR",
            "created_at": "2026-11-27T02:00:13Z",
            "error": "Model quota exceeded"
        }),
        // 14: Unknown step type
        json!({
            "step_index": 14,
            "source": "SYSTEM",
            "type": "FUTURE_STEP",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:14Z",
            "payload": "hello-future"
        }),
        // 15: Plain USER_INPUT without <USER_REQUEST> wrapper
        json!({
            "step_index": 15,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:15Z",
            "content": "Plain follow-up"
        }),
        // 16: PLANNER_RESPONSE at end with 2 tool calls, only 1 result follows -> second has no result
        json!({
            "step_index": 16,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:16Z",
            "model": "gemini-4-neon-high",
            "tool_calls": [
                {"name": "run_command", "args": {"CommandLine": "echo first"}},
                {"name": "run_command", "args": {"CommandLine": "echo still-running"}}
            ]
        }),
        json!({
            "step_index": 17,
            "source": "MODEL",
            "type": "GENERIC",
            "status": "DONE",
            "created_at": "2026-11-27T02:00:17Z",
            "content": "Created At: t\nCompleted At: t\nfirst\n"
        }),
    ];

    fs::write(&path, jsonl(&lines)).unwrap();
    let (parsed, adapted, session) = assemble_agy(conv_id, &path);

    assert_eq!(parsed.failed, 0);
    assert_eq!(adapted.records.len(), lines.len());
    assert!(adapted.uses_haa_tools);
    assert_eq!(
        adapted.unknown,
        vec![
            (UnknownKind::StepType, "FUTURE_STEP".to_owned(), 1),
            (UnknownKind::Tool, "mystery_tool".to_owned(), 1),
        ]
    );

    // Deterministic UUIDs, straight parentUuid chain, sessionId set, no cwd or title.
    let adapted_again = adapt(conv_id, &parsed);
    for (i, (a, b)) in adapted
        .records
        .iter()
        .zip(adapted_again.records.iter())
        .enumerate()
    {
        let uuid = a.entry.uuid.as_deref().expect("uuid");
        assert_eq!(uuid.len(), 36);
        assert_eq!(Some(uuid), b.entry.uuid.as_deref());
        assert_eq!(a.entry.session_id.as_deref(), Some(conv_id));
        assert!(a.entry.cwd.is_none());
        assert!(a.entry.ai_title.is_none());
        assert!(a.entry.custom_title.is_none());
        if i == 0 {
            assert!(a.entry.parent_uuid.is_none());
        } else {
            assert_eq!(a.entry.parent_uuid, adapted.records[i - 1].entry.uuid);
        }
    }

    // Check assembled summary.
    let agg = summarize(&session.skeleton);
    assert_eq!(
        agg.message_count, 4,
        "2 human prompts + 2 assistant replies"
    );
    assert_eq!(agg.tool_call_count, 12);
    assert_eq!(agg.tokens.input, 1000.0);
    assert_eq!(agg.tokens.cache_read, 250.0);
    assert_eq!(agg.tokens.output, 80.0);
    assert_eq!(agg.primary_model.as_deref(), Some("gemini-4-neon-high"));
    assert_eq!(
        agg.first_prompt.as_deref(),
        Some("Fix the bug. Note: keep `</USER_REQUEST>` inside backticks safe.")
    );

    // Check rendered display nodes.
    let t = main_t(&session, false);
    assert!(t.branch_points.is_empty());

    // Node 0: Human prompt with extracted <USER_REQUEST>.
    match &t.nodes[0].body {
        NodeBody::UserPrompt { text, origin, .. } => {
            assert_eq!(*origin, PromptOrigin::Human);
            assert_eq!(
                text,
                "Fix the bug. Note: keep `</USER_REQUEST>` inside backticks safe."
            );
        }
        other => panic!("expected UserPrompt, got {other:?}"),
    }

    // Node 1: Assistant with thinking, text, and 10 paired tool calls.
    let NodeBody::Assistant {
        model,
        blocks,
        usage,
        ..
    } = &t.nodes[1].body
    else {
        panic!("expected Assistant, got {:?}", t.nodes[1].body);
    };
    assert_eq!(model.as_deref(), Some("gemini-4-neon-high"));
    let u = usage.as_ref().expect("usage");
    assert_eq!(u.input, 1000.0);
    assert_eq!(u.cache_read, 250.0);
    assert_eq!(u.output, 80.0);
    assert!(matches!(
        &blocks[0],
        AssistantBlock::Thinking { text } if text == "Let me inspect and update the files."
    ));
    assert!(matches!(
        &blocks[1],
        AssistantBlock::Text { text } if text == "Running checks now."
    ));

    let calls = tool_calls(&t);
    assert_eq!(calls.len(), 12);

    // 0: run_command -> Bash
    assert_eq!(calls[0].name, "Bash");
    let v0: Value = serde_json::from_str(&calls[0].input_json).unwrap();
    assert_eq!(v0["command"], "cargo test");
    assert_eq!(v0["description"], "Run tests");
    assert_eq!(v0["agyOriginal"]["name"], "run_command");
    assert_eq!(v0["agyOriginal"]["args"]["WaitMsBeforeAsync"], 5000);
    let r0 = calls[0].result.as_ref().unwrap();
    assert!(!r0.is_error);
    assert!(r0.text.starts_with("Command exited with code 0."));
    assert!(!r0.text.contains("Created At:"));

    // 1: read_file -> Read (limit = 30 - 10 + 1 = 21), RUNNING result keeps text & is_error=false
    assert_eq!(calls[1].name, "Read");
    let v1: Value = serde_json::from_str(&calls[1].input_json).unwrap();
    assert_eq!(v1["file_path"], "/repo/src/lib.rs");
    assert_eq!(v1["offset"], 10);
    assert_eq!(v1["limit"], 21);
    assert_eq!(v1["agyOriginal"]["name"], "read_file");
    let r1 = calls[1].result.as_ref().unwrap();
    assert!(!r1.is_error);
    assert!(r1.text.starts_with("Tool is running as a background task"));

    // 2: edit_file -> Edit, ERROR result sets is_error=true
    assert_eq!(calls[2].name, "Edit");
    let v2: Value = serde_json::from_str(&calls[2].input_json).unwrap();
    assert_eq!(v2["file_path"], "/repo/src/lib.rs");
    assert_eq!(v2["old_string"], "old");
    assert_eq!(v2["new_string"], "new");
    assert_eq!(v2["agyOriginal"]["name"], "edit_file");
    let r2 = calls[2].result.as_ref().unwrap();
    assert!(r2.is_error);
    assert_eq!(r2.text, "Encountered error: target not found");

    // 3: write_file -> Write
    assert_eq!(calls[3].name, "Write");
    let v3: Value = serde_json::from_str(&calls[3].input_json).unwrap();
    assert_eq!(v3["file_path"], "/repo/out.txt");
    assert_eq!(v3["content"], "hello");
    assert_eq!(v3["agyOriginal"]["name"], "write_file");
    assert_eq!(calls[3].result.as_ref().unwrap().text, "Created file.");

    // 4: update_steps -> TodoWrite
    assert_eq!(calls[4].name, "TodoWrite");
    let v4: Value = serde_json::from_str(&calls[4].input_json).unwrap();
    assert_eq!(
        v4["todos"],
        json!([
            {"content": "Step A", "status": "completed"},
            {"content": "Step B", "status": "in_progress"},
            {"content": "Step C", "status": "pending"},
            {"content": "Step D", "status": "pending"},
        ])
    );
    assert_eq!(v4["agyOriginal"]["name"], "update_steps");

    // 5: search_web -> WebSearch
    assert_eq!(calls[5].name, "WebSearch");
    let v5: Value = serde_json::from_str(&calls[5].input_json).unwrap();
    assert_eq!(v5["query"], "rust serde rawvalue");
    assert_eq!(v5["agyOriginal"]["name"], "search_web");

    // 6: run_command missing CommandLine -> untranslated, no agyOriginal, error fallback from step.error
    assert_eq!(calls[6].name, "run_command");
    let v6: Value = serde_json::from_str(&calls[6].input_json).unwrap();
    assert!(v6.get("agyOriginal").is_none());
    assert_eq!(v6["toolSummary"], "missing command");
    let r6 = calls[6].result.as_ref().unwrap();
    assert!(r6.is_error);
    assert_eq!(r6.text, "missing CommandLine");

    // 7, 8, 9: untranslated tools keep original name & args without agyOriginal
    assert_eq!(calls[7].name, "manage_task");
    assert_eq!(calls[8].name, "mcp_haa_report");
    assert_eq!(calls[9].name, "mystery_tool");
    for c in &calls[7..=9] {
        let v: Value = serde_json::from_str(&c.input_json).unwrap();
        assert!(v.get("agyOriginal").is_none());
    }

    // System message, error message, unknown step, plain user prompt, trailing unmatched call.
    match &t.nodes[2].body {
        NodeBody::System {
            subtype,
            level,
            text,
        } => {
            assert_eq!(subtype, "informational");
            assert_eq!(level.as_deref(), Some("info"));
            assert_eq!(text, "Background task finished.");
        }
        other => panic!("expected info System, got {other:?}"),
    }
    match &t.nodes[3].body {
        NodeBody::System { level, text, .. } => {
            assert_eq!(level.as_deref(), Some("error"));
            assert_eq!(text, "Model quota exceeded");
        }
        other => panic!("expected error System, got {other:?}"),
    }
    match &t.nodes[4].body {
        NodeBody::Unknown {
            entry_type,
            raw_json,
        } => {
            assert_eq!(entry_type, "FUTURE_STEP");
            assert!(raw_json.contains("hello-future"));
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
    match &t.nodes[5].body {
        NodeBody::UserPrompt { text, origin, .. } => {
            assert_eq!(*origin, PromptOrigin::Human);
            assert_eq!(text, "Plain follow-up");
        }
        other => panic!("expected plain UserPrompt, got {other:?}"),
    }
    // Trailing call pair: first has result, second has none.
    assert_eq!(calls[10].result.as_ref().unwrap().text, "first\n");
    assert!(calls[11].result.is_none());
}

#[test]
fn overwritten_old_index_followed_by_valid_step_and_partial_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript_full.jsonl");

    let s0 = json!({"step_index": 0, "type": "USER_INPUT", "created_at": "2026-11-27T01:00:00Z", "content": "<USER_REQUEST>\nold prompt\n</USER_REQUEST>"});
    let s1 = json!({"step_index": 1, "type": "FUTURE_KIND", "created_at": "2026-11-27T01:00:01Z", "ver": "v1"});
    let s2 = json!({"step_index": 2, "type": "PLANNER_RESPONSE", "created_at": "2026-11-27T01:00:02Z", "content": "middle"});
    // Old index 1 overwritten near the end, then a newer valid step 3, then a half-written line.
    let s1_new = json!({"step_index": 1, "type": "FUTURE_KIND", "created_at": "2026-11-27T01:00:03Z", "ver": "v2-winner"});
    let s3 = json!({"step_index": 3, "type": "USER_INPUT", "created_at": "2026-11-27T01:00:04Z", "content": "<USER_REQUEST>\nafter overwrite\n</USER_REQUEST>"});
    let partial = r#"{"step_index":4,"type":"PLANNER_RESPONSE","content":"half"#;

    let body = format!("{s0}\n{s1}\n{s2}\n{s1_new}\n{s3}\n{partial}");
    fs::write(&path, &body).unwrap();

    let (parsed, adapted, session) = assemble_agy("conv-overwrite", &path);
    assert_eq!(parsed.lines, 5);
    assert_eq!(parsed.failed, 0);
    assert_eq!(parsed.consumed_to, (body.len() - partial.len()) as u64);
    assert_eq!(
        parsed.steps.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(parsed.steps[1].line_no, 4);
    assert_eq!(adapted.records.len(), 4);
    assert!(!adapted.uses_haa_tools);

    let t = main_t(&session, false);
    assert_eq!(t.nodes.len(), 4);
    // Node 1 is index 1 (sorted by step_index), and raw_line reads back the winning v2 line.
    match &t.nodes[1].body {
        NodeBody::Unknown {
            entry_type,
            raw_json,
        } => {
            assert_eq!(entry_type, "FUTURE_KIND");
            assert!(raw_json.contains("v2-winner"), "got {raw_json}");
            assert!(!raw_json.contains("\"v1\""));
        }
        other => panic!("expected Unknown for step 1, got {other:?}"),
    }
    match &t.nodes[2].body {
        NodeBody::Assistant { blocks, .. } => {
            assert!(matches!(&blocks[0], AssistantBlock::Text { text } if text == "middle"));
        }
        other => panic!("expected Assistant for step 2, got {other:?}"),
    }
    match &t.nodes[3].body {
        NodeBody::UserPrompt { text, .. } => assert_eq!(text, "after overwrite"),
        other => panic!("expected UserPrompt for step 3, got {other:?}"),
    }
}

#[test]
fn metadata_parsing_file_uri_decoding_fingerprint_and_haa_workspace() {
    let base = json!({
        "conversations": {
            "c1": {
                "summary": {
                    "Title": "  My Session  ",
                    "WorkspaceURIs": [
                        "file:///Users/dev/My%20Project",
                        "file:///Users/dev/%E4%B8%AD%E6%96%87/a+b",
                        "file://localhost/Users/dev/local",
                        "file:///Users/dev/bad%ZZ",
                        "file:///Users/dev/trunc%2",
                        "file:///Users/dev/nul%00path",
                        "file://remote-host/share",
                        "https://example.com/repo"
                    ],
                    "StepCount": 10
                }
            },
            "c2": {
                "summary": {
                    "Title": "   ",
                    "WorkspaceURIs": []
                }
            },
            "c3": {
                "summary": {
                    "WorkspaceURIs": ["file:///Users/dev/.haa/agy-hooks"]
                }
            },
            "c4": {
                "summary": {
                    "WorkspaceURIs": ["file:///Users/dev/.paaa/worktrees/w1"]
                }
            },
            "c5": {
                "summary": {
                    "WorkspaceURIs": ["file:///Users/dev/.haa-other/proj"]
                }
            }
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("conversation_metadata.json");
    fs::write(&p, serde_json::to_vec(&base).unwrap()).unwrap();
    let m1 = read_metadata(&p).unwrap();

    let c1 = &m1["c1"];
    assert_eq!(c1.title.as_deref(), Some("My Session"));
    assert_eq!(
        c1.workspaces,
        vec![
            PathBuf::from("/Users/dev/My Project"),
            PathBuf::from("/Users/dev/中文/a+b"),
            PathBuf::from("/Users/dev/local"),
        ]
    );
    assert_eq!(m1["c2"].title, None);
    assert!(m1["c2"].workspaces.is_empty());

    // Fingerprint ignores unrelated summary fields and changes when Title or WorkspaceURIs change.
    let mut same_fields = base.clone();
    same_fields["conversations"]["c1"]["summary"]["StepCount"] = json!(999);
    same_fields["conversations"]["c1"]["summary"]["Extra"] = json!("ignored");
    let m2 = parse_metadata(&serde_json::to_vec(&same_fields).unwrap());
    assert_eq!(m1["c1"].fingerprint, m2["c1"].fingerprint);

    let mut title_changed = base.clone();
    title_changed["conversations"]["c1"]["summary"]["Title"] = json!("Renamed");
    let m3 = parse_metadata(&serde_json::to_vec(&title_changed).unwrap());
    assert_ne!(m1["c1"].fingerprint, m3["c1"].fingerprint);

    let mut ws_changed = base.clone();
    ws_changed["conversations"]["c1"]["summary"]["WorkspaceURIs"] =
        json!(["file:///Users/dev/other"]);
    let m4 = parse_metadata(&serde_json::to_vec(&ws_changed).unwrap());
    assert_ne!(m1["c1"].fingerprint, m4["c1"].fingerprint);

    // haa workspace check: under haa_home or sibling .paaa, not prefix-similar sibling.
    let haa_home = Path::new("/Users/dev/.haa");
    assert!(!is_haa_workspace(&m1["c1"], haa_home));
    assert!(is_haa_workspace(&m1["c3"], haa_home));
    assert!(is_haa_workspace(&m1["c4"], haa_home));
    assert!(!is_haa_workspace(&m1["c5"], haa_home));
}

#[test]
fn haa_tools_detection_covers_haa_and_paaa_prefixes() {
    let make = |tool: &str| {
        let line = json!({
            "step_index": 0,
            "type": "PLANNER_RESPONSE",
            "tool_calls": [{"name": tool, "args": {}}]
        });
        adapt("c", &parse(format!("{line}\n").as_bytes()))
    };
    assert!(make("mcp_haa_report").uses_haa_tools);
    assert!(make("mcp_haa_report").unknown.is_empty());
    assert!(make("mcp_paaa_ask").uses_haa_tools);
    assert!(make("mcp_paaa_ask").unknown.is_empty());
    assert!(!make("mcp_github_search").uses_haa_tools);
    assert!(make("mcp_github_search").unknown.is_empty());
}

#[test]
#[ignore = "real agy data probe (read-only)"]
fn real_agy_data_probe() {
    let root = std::env::var_os("CODECK_AGY_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME set"))
                .join(".gemini")
                .join("antigravity-cli")
        });
    let brain = brain_dir(&root);
    let mut files = Vec::new();
    if let Ok(rd) = fs::read_dir(&brain) {
        for entry in rd.filter_map(|e| e.ok()) {
            let id = entry.file_name().to_string_lossy().to_string();
            let tp = transcript_path(&root, &id);
            if tp.is_file() {
                files.push((id, tp));
            }
        }
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "expected at least one agy transcript_full.jsonl under {}",
        brain.display()
    );

    let mut total_failed = 0u64;
    let mut total_calls = 0u64;
    let mut paired_calls = 0u64;
    let mut unknown_totals: BTreeMap<(UnknownKind, String), u64> = BTreeMap::new();

    for (id, path) in &files {
        let parsed = read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if parsed.failed > 0 {
            eprintln!(
                "BAD LINES in {}: failed={}, first_error={:?}",
                path.display(),
                parsed.failed,
                parsed.first_error
            );
        }
        total_failed += u64::from(parsed.failed);

        let adapted = adapt(id, &parsed);
        for (kind, name, count) in adapted.unknown {
            *unknown_totals.entry((kind, name)).or_insert(0) += u64::from(count);
        }

        let loaded = LoadedFile {
            path: path.clone(),
            role: FileRole::Main,
            agent_id: None,
            records: adapted.records,
        };
        let session_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let session = assemble(id, session_dir, vec![loaded], SessionSidecar::default());
        let t = main_t(&session, false);
        for c in tool_calls(&t) {
            total_calls += 1;
            if c.result.is_some() {
                paired_calls += 1;
            }
        }
    }

    let rate = if total_calls == 0 {
        100.0
    } else {
        (paired_calls as f64 / total_calls as f64) * 100.0
    };
    eprintln!(
        "agy probe: files={}, failed_lines={}, unknown_kinds={}, tool_calls={}/{} ({:.2}% paired)",
        files.len(),
        total_failed,
        unknown_totals.len(),
        paired_calls,
        total_calls,
        rate
    );
    for ((kind, name), count) in &unknown_totals {
        eprintln!("  unknown {kind:?}: {name} x {count}");
    }

    assert_eq!(
        total_failed, 0,
        "expected 0 failed complete lines across real agy transcripts"
    );
}

#[test]
fn ids_from_lock_names_and_paths_must_be_one_plain_component() {
    use std::path::Path;
    let root = Path::new("/r");
    assert!(transcript::agy::is_conversation_id(
        "8c54e127-d8c4-55e2-9034-566dac2ca233"
    ));
    for bad in ["", ".", "..", "a/b", "a\\b"] {
        assert!(!transcript::agy::is_conversation_id(bad), "{bad:?}");
    }
    assert_eq!(
        transcript::agy::session_of_presence(root, Path::new("/r/presence/..lock")),
        None
    );
    assert_eq!(
        transcript::agy::session_of_presence(root, Path::new("/r/presence/...lock")),
        None
    );
    assert_eq!(
        transcript::agy::session_of_presence(root, Path::new("/r/presence/abc.lock")).as_deref(),
        Some("abc")
    );
}
