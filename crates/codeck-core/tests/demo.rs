//! The demo data root (`demo/root`, built by `scripts/demo/build.ts`): it indexes cleanly and its
//! Sessions show every kind of message the conversation view draws (the design canvas's 「2 · 对话记录」).
mod common;

use std::collections::BTreeSet;
use std::path::Path;

use codeck_core::model::{TranscriptRequest, TranscriptScope};
use common::copy_dir_all;
use common::engine::Env;
use serde_json::Value;

fn demo_env() -> Env {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/root");
    let tmp = tempfile::tempdir().unwrap();
    copy_dir_all(&src, tmp.path()).unwrap();
    Env::scanned(tmp)
}

fn transcript(env: &Env, sid: &str, scope: TranscriptScope) -> Value {
    transcript_with(env, sid, scope, true)
}

fn transcript_with(env: &Env, sid: &str, scope: TranscriptScope, include_hidden: bool) -> Value {
    let t = env
        .engine
        .get_transcript(&TranscriptRequest {
            session_id: sid.into(),
            scope,
            branch_choices: Default::default(),
            include_hidden,
        })
        .unwrap();
    serde_json::to_value(t).unwrap()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

/// What a transcript shows, as feature names (`prompt.human`, `tool.Bash.failed`, `branch`…).
fn features(t: &Value, out: &mut BTreeSet<String>) {
    let mut add = |f: String| {
        out.insert(f);
    };
    for n in t["nodes"].as_array().unwrap() {
        let b = &n["body"];
        let kind = s(b, "kind");
        match kind.as_str() {
            "userPrompt" => {
                let o = &b["origin"];
                let ok = s(o, "kind");
                add(format!("prompt.{ok}"));
                if ok == "command" {
                    add(if s(o, "name") == "!" {
                        "prompt.bang".into()
                    } else {
                        "prompt.slash".into()
                    });
                }
                if ok == "peer" {
                    add(format!("prompt.peer.{}", s(o, "source")));
                }
                if ok == "taskNotification" {
                    add(format!("notification.{}", s(o, "status")));
                }
                if b["images"].as_array().is_some_and(|a| !a.is_empty()) {
                    add("prompt.image".into());
                }
                if s(b, "text").contains('`') {
                    add("prompt.code".into());
                }
            }
            "assistant" => {
                if b["isApiError"].as_bool() == Some(true) {
                    add("assistant.apiError".into());
                }
                for blk in b["blocks"].as_array().unwrap() {
                    match s(blk, "kind").as_str() {
                        "thinking" if !s(blk, "text").is_empty() => {
                            add("assistant.thinking".into())
                        }
                        "text" => {
                            let tx = s(blk, "text");
                            if tx.contains("\n|") {
                                add("markdown.table".into());
                            }
                            if tx.contains("```") {
                                add("markdown.code".into());
                            }
                            if tx.contains("](http") {
                                add("markdown.link".into());
                            }
                        }
                        "toolCall" => {
                            let call = blk;
                            let name = s(call, "name");
                            let name = if name.starts_with("mcp__") {
                                "mcp".into()
                            } else {
                                name
                            };
                            add(format!("tool.{name}"));
                            let r = &call["result"];
                            if r["isError"].as_bool() == Some(true) {
                                add(format!("tool.{name}.failed"));
                            }
                            if s(r, "extraJson").contains("backgroundTaskId") {
                                add(format!("tool.{name}.background"));
                            }
                            if s(call, "inputJson").contains("\"replace_all\":true") {
                                add("tool.Edit.replaceAll".into());
                            }
                        }
                        other => add(format!("block.{other}")),
                    }
                }
            }
            "system" => add(format!("system.{}", s(b, "subtype"))),
            other => add(format!("node.{other}")),
        }
    }
    if !t["branchPoints"].as_array().unwrap().is_empty() {
        add("branch".into());
    }
    if t["inherited"].is_object() {
        add("fork".into());
    }
    if !t["workflows"].as_array().unwrap().is_empty() {
        add("workflow".into());
    }
    for r in t["subagents"].as_array().unwrap() {
        add("subagent".into());
        if r["isAsync"].as_bool() == Some(true) {
            add("subagent.background".into());
        }
        if !r["teammate"].is_null() && r["teammate"] != Value::Bool(false) {
            add("subagent.teammate".into());
        }
    }
    if t["subagents"].as_array().unwrap().len() > 1 {
        add("subagent.parallel".into());
    }
}

/// Every message type of the conversation view the demo must show.
const REQUIRED: &[&str] = &[
    "prompt.human",
    "prompt.code",
    "prompt.slash",
    "prompt.bang",
    "prompt.image",
    "prompt.commandOutput",
    "prompt.meta",
    "notification.completed",
    "notification.failed",
    "assistant.thinking",
    "assistant.apiError",
    "markdown.table",
    "markdown.code",
    "markdown.link",
    "tool.Bash",
    "tool.Bash.failed",
    "tool.Bash.background",
    "tool.Edit",
    "tool.Edit.failed",
    "tool.Edit.replaceAll",
    "tool.Write",
    "tool.Read",
    "tool.Grep",
    "tool.TaskCreate",
    "tool.TaskUpdate",
    "tool.WebFetch",
    "tool.WebSearch",
    "tool.mcp",
    "tool.AskUserQuestion",
    "tool.Agent",
    "subagent",
    "subagent.background",
    "subagent.parallel",
    "node.compactBoundary",
    "system.informational",
    "system.api_error",
    "system.turn_duration",
    "system.stop_hook_summary",
    "node.attachment",
    "branch",
    "fork",
    "workflow",
    "tool.Workflow",
    "notification.completed",
    "prompt.teammate",
    "prompt.peer",
    "prompt.peer.session",
    "subagent.teammate",
    "tool.SendMessage",
];

#[test]
fn demo_root_indexes_cleanly_and_covers_every_message_type() {
    let env = demo_env();
    let d = serde_json::to_value(env.engine.diagnostics().unwrap()).unwrap();
    for key in [
        "unknownEntryTypes",
        "unknownBlockTypes",
        "unknownSystemSubtypes",
        "unknownTools",
        "unrecognizedMessages",
    ] {
        assert!(
            d[key].as_array().is_none_or(|a| a.is_empty()),
            "{key}: {}",
            d[key]
        );
    }
    assert!(
        d["failedLines"].as_u64().unwrap_or(0) == 0,
        "failed lines: {}",
        d["failedLines"]
    );

    let sessions = env.engine.list_sessions(&Default::default()).unwrap();
    let mut found = BTreeSet::new();
    for sess in &sessions {
        let t = transcript(&env, &sess.id, TranscriptScope::Main);
        features(&t, &mut found);
        for r in t["subagents"].as_array().unwrap() {
            let agent_id = s(r, "agentId");
            let sub = transcript(
                &env,
                &sess.id,
                TranscriptScope::Subagent {
                    agent_id: agent_id.clone(),
                },
            );
            assert!(
                !sub["nodes"].as_array().unwrap().is_empty(),
                "empty Subagent Run {agent_id}"
            );
        }
    }
    // What shows without 「显示系统消息」: system injections stay hidden, messages from other agents do not.
    let mut visible = BTreeSet::new();
    for sess in &sessions {
        features(
            &transcript_with(&env, &sess.id, TranscriptScope::Main, false),
            &mut visible,
        );
    }
    for f in [
        "prompt.peer",
        "prompt.teammate",
        "notification.completed",
        "system.api_error",
    ] {
        assert!(visible.contains(f), "{f} hidden by default");
    }
    for f in ["prompt.meta", "node.attachment", "system.turn_duration"] {
        assert!(!visible.contains(f), "{f} shown by default");
    }
    assert_eq!(sessions.len(), 4, "sessions listed: {}", sessions.len());
    eprintln!("demo features: {found:?}");
    let missing: Vec<_> = REQUIRED.iter().filter(|f| !found.contains(**f)).collect();
    assert!(missing.is_empty(), "missing in demo/root: {missing:?}");
}

/// Like the fixtures: no home directory other than `/Users/dev`, nothing from `CODECK_FIXTURE_DENYLIST`.
#[test]
fn demo_files_hold_no_private_data() {
    let extra = std::env::var("CODECK_FIXTURE_DENYLIST")
        .unwrap_or_default()
        .to_lowercase();
    let bad: Vec<&str> = extra
        .split(',')
        .map(str::trim)
        .filter(|t| t.len() > 2)
        .collect();
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo");
    let mut stack = vec![base.join("root"), base.join("shapes"), base.join("story")];
    let mut files = 0;
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            stack.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        files += 1;
        let text = std::fs::read_to_string(&p).unwrap().to_lowercase();
        for (i, _) in text.match_indices("/users/") {
            assert!(
                text[i..].starts_with("/users/dev"),
                "{} has a non-dev home path",
                p.display()
            );
        }
        for b in &bad {
            assert!(!text.contains(b), "{} contains {b}", p.display());
        }
    }
    assert!(files > 40, "only {files} demo files");
}
