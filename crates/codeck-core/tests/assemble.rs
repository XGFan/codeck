//! Assembly (§3.4 steps 5–12): fragments, pairing, hidden entries, fork, titles, subagents,
//! workflows, persisted output and images. Targeted asserts on fixtures; no snapshots.
mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use codeck_core::assemble::{
    AssembledSession, LoadedFile, SessionSidecar, SubagentMetaFile, WorkflowFiles, assemble,
    build_skeleton, content, find, summarize, transcript,
};
use codeck_core::model::{
    AssistantBlock, BranchChoice, DetailSource, FileRole, FindLocation, FindRequest, ForkOrigin,
    ImageRequest, NodeBody, PromptOrigin, TitleSource, ToolCall, ToolDetailRequest, ToolPart,
    Transcript, TranscriptRequest, TranscriptScope,
};
use codeck_core::parse::{parse_bytes, read_entries};
use common::{fixture_dir, ids};

// ---- loading (mirrors what the engine gathers; read-only) ----

fn load(path: &Path, role: FileRole, agent_id: Option<String>) -> LoadedFile {
    LoadedFile {
        path: path.to_path_buf(),
        role,
        agent_id,
        records: read_entries(path, 0).expect("read jsonl").records,
    }
}

fn read_opt(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok()
}

/// Sidecar of `<session_dir>`: subagent metas (plain and workflow) and workflow files.
fn load_sidecar(session_dir: &Path) -> SessionSidecar {
    let mut sidecar = SessionSidecar::default();
    let sub = session_dir.join("subagents");
    let mut add_agents = |dir: &Path, role: FileRole, run: Option<String>| {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut ids: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let stem = name
                    .strip_suffix(".meta.json")
                    .or_else(|| name.strip_suffix(".jsonl"))?;
                stem.strip_prefix("agent-").map(str::to_owned)
            })
            .collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            let jsonl = dir.join(format!("agent-{id}.jsonl"));
            sidecar.subagent_metas.push(SubagentMetaFile {
                agent_id: id.clone(),
                role,
                workflow_run_id: run.clone(),
                transcript_path: jsonl.is_file().then_some(jsonl),
                meta_json: read_opt(&dir.join(format!("agent-{id}.meta.json"))),
            });
        }
    };
    add_agents(&sub, FileRole::Subagent, None);
    let mut runs: Vec<String> = Vec::new();
    if let Ok(rd) = fs::read_dir(sub.join("workflows")) {
        for e in rd.filter_map(|e| e.ok()) {
            let run = e.file_name().to_string_lossy().to_string();
            add_agents(&e.path(), FileRole::WorkflowSubagent, Some(run.clone()));
            runs.push(run);
        }
    }
    if let Ok(rd) = fs::read_dir(session_dir.join("workflows")) {
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(run) = name.strip_suffix(".json") {
                runs.push(run.to_owned());
            }
        }
    }
    runs.sort();
    runs.dedup();
    for run in runs {
        sidecar.workflows.push(WorkflowFiles {
            workflow_json: read_opt(&session_dir.join("workflows").join(format!("{run}.json"))),
            journal_jsonl: read_opt(&sub.join("workflows").join(&run).join("journal.jsonl")),
            run_id: run,
        });
    }
    sidecar
}

fn open(scenario: &str, dir: &str, sid: &str) -> AssembledSession {
    let project = fixture_dir(scenario).join("projects").join(dir);
    let main = load(&project.join(format!("{sid}.jsonl")), FileRole::Main, None);
    let session_dir = project.join(sid);
    let sidecar = load_sidecar(&session_dir);
    assemble(sid, &session_dir, vec![main], sidecar)
}

fn agent_file(s: &AssembledSession, agent_id: &str) -> LoadedFile {
    let m = s
        .sidecar
        .subagent_metas
        .iter()
        .find(|m| m.agent_id == agent_id)
        .expect("agent meta");
    load(
        m.transcript_path.as_ref().expect("agent transcript"),
        m.role,
        Some(agent_id.to_owned()),
    )
}

fn request(scope: TranscriptScope, include_hidden: bool) -> TranscriptRequest {
    TranscriptRequest {
        session_id: "s".into(),
        scope,
        branch_choices: Vec::new(),
        include_hidden,
    }
}

fn main_t(s: &AssembledSession, include_hidden: bool) -> Transcript {
    transcript(s, &request(TranscriptScope::Main, include_hidden), None).unwrap()
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

fn call<'a>(t: &'a Transcript, tool_use_id: &str) -> &'a ToolCall {
    tool_calls(t)
        .into_iter()
        .find(|c| c.tool_use_id == tool_use_id)
        .unwrap_or_else(|| panic!("no tool call {tool_use_id}"))
}

fn from_lines(lines: &[&str]) -> LoadedFile {
    let text = lines.join("\n") + "\n";
    LoadedFile {
        path: PathBuf::from("inline.jsonl"),
        role: FileRole::Main,
        agent_id: None,
        records: parse_bytes(text.as_bytes(), 0).records,
    }
}

// ---- fragments, tokens, pairing, hidden ----

#[test]
fn fragments_merge_into_one_node_and_tokens_count_once() {
    let s = open("basic", ids::BASIC_DIR, ids::BASIC);
    let a = summarize(&s.skeleton);
    assert_eq!(
        a.message_count, 20,
        "4 human prompts + 16 assistant messages"
    );
    assert_eq!(a.tool_call_count, 18);
    assert_eq!(a.tokens.input, 51.0);
    assert_eq!(a.tokens.output, 5151.0);
    assert_eq!(a.tokens.cache_read, 1_761_742.0);
    assert_eq!(a.tokens.cache_creation, 33_071.0);

    let t = main_t(&s, false);
    let assistants: Vec<_> = t
        .nodes
        .iter()
        .filter(|n| matches!(n.body, NodeBody::Assistant { .. }))
        .collect();
    assert_eq!(assistants.len(), 16);
    // The 5-fragment parallel group: one text + 4 tool calls, id = first fragment.
    let group = assistants
        .iter()
        .find(|n| n.id.starts_with("cd574975"))
        .unwrap();
    let NodeBody::Assistant { blocks, usage, .. } = &group.body else {
        unreachable!()
    };
    assert_eq!(blocks.len(), 5);
    assert!(matches!(blocks[0], AssistantBlock::Text { .. }));
    assert!(
        blocks[1..]
            .iter()
            .all(|b| matches!(b, AssistantBlock::ToolCall(_)))
    );
    // Usage is repeated on each fragment and taken once (max per field), not summed.
    let sk = &s.skeleton;
    let frags = &sk.fragments[sk.nodes[sk.by_uuid[&group.id]].message_id.as_ref().unwrap()];
    assert_eq!(frags.len(), 5);
    let max = |f: fn(&codeck_core::model::TokenTotals) -> f64| {
        frags
            .iter()
            .map(|&i| f(sk.nodes[i].usage.as_ref().unwrap()))
            .fold(0.0, f64::max)
    };
    let u = usage.as_ref().unwrap();
    assert_eq!(u.input, max(|t| t.input));
    assert_eq!(u.output, max(|t| t.output));
    assert_eq!(u.cache_read, max(|t| t.cache_read));
    assert_eq!(u.cache_creation, max(|t| t.cache_creation));
    // Fragment uuids never appear as nodes of their own.
    assert!(!t.nodes.iter().any(|n| n.id.starts_with("c28542b7")));
    // Empty thinking blocks are dropped.
    let first = assistants
        .iter()
        .find(|n| n.id.starts_with("8cf7fed3"))
        .unwrap();
    let NodeBody::Assistant { blocks, .. } = &first.body else {
        unreachable!()
    };
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, AssistantBlock::Thinking { .. }))
    );
    let fragment = s.skeleton.by_uuid[&first.id] + 2;
    assert_eq!(
        s.skeleton.nodes[s.skeleton.display_index(fragment)].uuid,
        first.id
    );
}

#[test]
fn off_path_results_are_paired_and_errors_propagate() {
    let s = open("basic", ids::BASIC_DIR, ids::BASIC);
    let t = main_t(&s, false);
    let calls = tool_calls(&t);
    assert_eq!(calls.len(), 18);
    assert!(
        calls.iter().all(|c| c.result.is_some()),
        "every call is paired"
    );
    // Results 30–32 hang off fragments that are not on the raw path.
    for id in [
        "toolu_01QX4E5bFF3YX8Sfh16X6t4B",
        "toolu_013vSm4UccCPVT9mHiHD1CNS",
        "toolu_015W6eLsjRuGEF19BRNGytmW",
        "toolu_01RVofdvgpZrbyVAHNxDxUDT",
    ] {
        let r = call(&t, id).result.as_ref().unwrap();
        assert!(!r.text.is_empty());
        assert!(!r.is_error);
    }
    let failed = call(&t, "toolu_e2ccd9f8e29e53a88b3896");
    assert!(failed.result.as_ref().unwrap().is_error);
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.result.as_ref().unwrap().is_error)
            .count(),
        1
    );
    // Bash extra_json keeps the toolUseResult subset.
    let ok_bash = call(&t, "toolu_01BmgZxiLdtF1mBgVjtPMSyv")
        .result
        .as_ref()
        .unwrap();
    let extra: serde_json::Value =
        serde_json::from_str(ok_bash.extra_json.as_ref().unwrap()).unwrap();
    assert!(extra.get("stdout").is_some() && extra.get("interrupted").is_some());
    // Absorbed tool_result entries never become nodes.
    assert!(
        !t.nodes
            .iter()
            .any(|n| n.id.starts_with("7f17d54c") || n.id.starts_with("a85a8bba"))
    );
    // Tool input is valid capped JSON.
    let input: serde_json::Value =
        serde_json::from_str(&call(&t, "toolu_01WzSKuC9zCMnYY1XKS41H6N").input_json).unwrap();
    assert!(input.is_object());
}

#[test]
fn hidden_entries_follow_the_default_rules() {
    let s = open("basic", ids::BASIC_DIR, ids::BASIC);
    let t = main_t(&s, false);
    assert!(t.hidden_count > 0);
    assert!(t.nodes.iter().all(|n| !n.hidden));
    assert!(
        !t.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::Attachment { .. }))
    );
    let subtypes: Vec<&str> = t
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::System { subtype, .. } => Some(subtype.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(subtypes, ["informational", "api_error", "future_notice"]);
    let api_error = t
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::System { subtype, text, .. } if subtype == "api_error" => Some(text),
            _ => None,
        })
        .unwrap();
    assert!(api_error.contains("Connection error"));
    let unknown = t
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Unknown {
                entry_type,
                raw_json,
            } => Some((entry_type, raw_json)),
            _ => None,
        })
        .expect("unknown entry type is shown");
    assert_eq!(unknown.0, "hologram");
    assert!(unknown.1.contains("frame"));
    assert!(t.nodes.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { blocks, .. } if blocks.iter().any(|b| matches!(b, AssistantBlock::Unknown { block_type, .. } if block_type == "hologram_block")))));
    let origins: Vec<&PromptOrigin> = t
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::UserPrompt { origin, .. } => Some(origin),
            _ => None,
        })
        .collect();
    assert_eq!(origins.len(), 5);
    assert!(origins.contains(&&PromptOrigin::Command {
        name: "/release".into(),
        args: "v1.2.0".into()
    }));
    // The `local_command` system entry is the command's output.
    assert!(origins.contains(&&PromptOrigin::CommandOutput));

    let all = main_t(&s, true);
    assert_eq!(all.hidden_count, t.hidden_count);
    assert_eq!(all.nodes.len(), t.nodes.len() + t.hidden_count as usize);
    let hidden: Vec<_> = all.nodes.iter().filter(|n| n.hidden).collect();
    assert!(
        hidden
            .iter()
            .any(|n| matches!(n.body, NodeBody::Attachment { .. }))
    );
    assert!(hidden.iter().any(
        |n| matches!(&n.body, NodeBody::System { subtype, .. } if subtype == "turn_duration")
    ));
    assert!(hidden.iter().any(
        |n| matches!(&n.body, NodeBody::System { subtype, .. } if subtype == "stop_hook_summary")
    ));
    assert!(hidden.iter().any(|n| matches!(
        &n.body,
        NodeBody::UserPrompt {
            origin: PromptOrigin::Meta,
            ..
        }
    )));
}

// ---- fork ----

#[test]
fn fork_inherited_range_ends_at_the_last_inherited_node() {
    let mut s = open("fork", ids::FORK_DIR, ids::FORK_CHILD);
    s.sidecar.fork_origin = Some(ForkOrigin {
        session_id: ids::FORK_ORIGIN.into(),
        title: Some("Mac GUI architecture".into()),
        fork_point_id: None,
    });
    let a = summarize(&s.skeleton);
    assert_eq!(a.fork_origin_id.as_deref(), Some(ids::FORK_ORIGIN));
    assert_eq!(a.title, "Mac GUI architecture (Branch)");
    assert_eq!(a.title_source, TitleSource::Custom);
    assert!(
        a.fork_point_uuid
            .as_deref()
            .unwrap()
            .starts_with("4be1f323")
    );

    let t = main_t(&s, false);
    let r = t.inherited.as_ref().expect("inherited range");
    assert_eq!(r.origin_session_id, ids::FORK_ORIGIN);
    assert_eq!(r.origin_title.as_deref(), Some("Mac GUI architecture"));
    assert!(
        r.last_inherited_id.starts_with("02476eb7"),
        "{}",
        r.last_inherited_id
    );
    let pos = t
        .nodes
        .iter()
        .position(|n| n.id == r.last_inherited_id)
        .unwrap();
    assert!(t.nodes[..=pos].iter().all(|n| n.inherited));
    assert!(t.nodes[pos + 1..].iter().all(|n| !n.inherited));
    assert_eq!(r.count as usize, pos + 1);
    assert!(t.nodes[pos + 1].id.starts_with("2f0828d3"));

    let all = main_t(&s, true);
    let r = all.inherited.unwrap();
    assert!(r.last_inherited_id.starts_with("4be1f323"));

    // Index fallback: no `forkedFrom` (the origin file itself), fork point known from the index.
    let mut origin = open("fork", ids::FORK_DIR, ids::FORK_ORIGIN);
    assert!(main_t(&origin, false).inherited.is_none());
    let point = origin
        .skeleton
        .nodes
        .iter()
        .find(|n| n.uuid.starts_with("4be1f323"))
        .unwrap()
        .uuid
        .clone();
    origin.sidecar.fork_origin = Some(ForkOrigin {
        session_id: "other".into(),
        title: None,
        fork_point_id: Some(point),
    });
    let t = main_t(&origin, false);
    let r = t.inherited.unwrap();
    assert_eq!(r.origin_session_id, "other");
    assert!(r.last_inherited_id.starts_with("02476eb7"));
}

// ---- titles ----

#[test]
fn title_priority_custom_then_ai_then_first_prompt() {
    let title = |lines: &[&str]| summarize(&build_skeleton(&[from_lines(lines)]));
    let prompt = r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"Fix   the\nbuild"}}"#;
    let meta = r#"{"type":"user","uuid":"u0","parentUuid":null,"isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#;
    let note = r#"{"type":"user","uuid":"u00","parentUuid":null,"origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification><task-id>t</task-id></task-notification>"}}"#;
    let ai = r#"{"type":"ai-title","aiTitle":"AI title"}"#;
    let ai2 = r#"{"type":"ai-title","aiTitle":"AI title 2"}"#;
    let custom = r#"{"type":"custom-title","customTitle":"Custom"}"#;

    let a = title(&[meta, note, prompt, ai, custom, ai2]);
    assert_eq!(
        (a.title.as_str(), a.title_source),
        ("Custom", TitleSource::Custom)
    );
    let a = title(&[prompt, ai, ai2]);
    assert_eq!(
        (a.title.as_str(), a.title_source),
        ("AI title 2", TitleSource::Ai)
    );
    let a = title(&[meta, note, prompt]);
    assert_eq!(
        (a.title.as_str(), a.title_source),
        ("Fix the build", TitleSource::FirstPrompt)
    );
    assert_eq!(a.first_prompt.as_deref(), Some("Fix the build"));
    let a = title(&[meta]);
    assert_eq!(
        (a.title.as_str(), a.title_source),
        ("", TitleSource::Untitled)
    );
    assert!(a.is_empty);

    let cmd = r#"{"type":"user","uuid":"c1","parentUuid":null,"message":{"role":"user","content":"<command-message>release</command-message>\n<command-name>/release</command-name>\n<command-args>v1.2.0</command-args>"}}"#;
    assert_eq!(title(&[cmd]).title, "/release v1.2.0");
    let long = format!(
        r#"{{"type":"user","uuid":"l1","parentUuid":null,"message":{{"role":"user","content":"{}"}}}}"#,
        "長".repeat(300)
    );
    assert_eq!(title(&[long.as_str()]).title.chars().count(), 120);

    let basic = summarize(&open("basic", ids::BASIC_DIR, ids::BASIC).skeleton);
    assert_eq!(basic.title, "retry-backoff");
    assert_eq!(
        basic.first_prompt.as_deref(),
        Some("Add retry with exponential backoff to the HTTP client in src/client.rs")
    );
    let branch = summarize(&open("branch", ids::BRANCH_DIR, ids::BRANCH).skeleton);
    assert_eq!(
        (branch.title.as_str(), branch.title_source),
        ("Parse ISO dates", TitleSource::Ai)
    );
}

// ---- subagents ----

const SYNC: &str = "a11b19d522e5e3835";
const ASYNC: &str = "a87b5e24025a157e4";
const TEAMMATE: &str = "agui-impl-16f075d476ac2e52";
const NESTED: &str = "a1e2a06b87a0850ff";
const ORPHAN: &str = "a0rphan000000000001";

#[test]
fn subagents_link_by_tool_use_id_name_and_parent() {
    let s = open("subagents", ids::SUBAGENTS_DIR, ids::SUBAGENTS);
    let t = main_t(&s, false);
    assert_eq!(t.orphan_subagent_ids, [ORPHAN]);
    let run = |id: &str| t.subagents.iter().find(|r| r.agent_id == id).unwrap();

    // meta.toolUseId (sync)
    let sync_call = call(&t, "toolu_011zFxQKA9ZqiQjNSrpVfbB7");
    assert_eq!(sync_call.subagent_id.as_deref(), Some(SYNC));
    assert!(!run(SYNC).is_async);
    assert_eq!(run(SYNC).status.as_deref(), Some("completed"));
    assert_eq!(run(SYNC).agent_type.as_deref(), Some("codex:codex-rescue"));
    assert!(run(SYNC).final_text.is_some());

    // async + task-notification backlink
    let async_call = call(&t, "toolu_01CZKpZhFGtnywQwyH8oUrsj");
    assert_eq!(async_call.subagent_id.as_deref(), Some(ASYNC));
    assert!(run(ASYNC).is_async);
    let note_id = async_call
        .notification_node_id
        .as_deref()
        .expect("notification link");
    assert!(note_id.starts_with("5477cbcc"));
    let note = t.nodes.iter().find(|n| n.id == note_id).unwrap();
    match &note.body {
        NodeBody::UserPrompt {
            origin:
                PromptOrigin::TaskNotification {
                    tool_use_id,
                    task_id,
                    ..
                },
            ..
        } => {
            assert_eq!(
                tool_use_id.as_deref(),
                Some("toolu_01CZKpZhFGtnywQwyH8oUrsj")
            );
            assert_eq!(task_id.as_deref(), Some(ASYNC));
        }
        b => panic!("unexpected {b:?}"),
    }
    // A background run's result is the notification's <result>, not the launch tool_result.
    assert_eq!(run(ASYNC).status.as_deref(), Some("completed"));
    assert!(
        run(ASYNC)
            .final_text
            .as_deref()
            .is_some_and(|t| t.starts_with("dolore minim exercitation"))
    );

    // meta.name == toolUseResult.name (teammate without toolUseId)
    let team_call = call(&t, "toolu_01TdePUUDtAB6J6JGdhnLudR");
    assert_eq!(team_call.subagent_id.as_deref(), Some(TEAMMATE));
    assert_eq!(
        run(TEAMMATE).tool_use_id.as_deref(),
        Some("toolu_01TdePUUDtAB6J6JGdhnLudR")
    );

    // nested: parentAgentId, toolUseId resolves inside the parent's file
    let nested = run(NESTED);
    assert_eq!(nested.parent_agent_id.as_deref(), Some(TEAMMATE));
    assert_eq!(nested.spawn_depth, 1);
    assert_eq!(
        nested.tool_use_id.as_deref(),
        Some("toolu_01XeTmLfyUTxt5797EY8mD2v")
    );
    assert!(
        !s.subagent_by_tool
            .contains_key("toolu_01XeTmLfyUTxt5797EY8mD2v")
    );
    let team_file = agent_file(&s, TEAMMATE);
    let sub = transcript(
        &s,
        &request(
            TranscriptScope::Subagent {
                agent_id: TEAMMATE.into(),
            },
            false,
        ),
        Some(&team_file),
    )
    .unwrap();
    assert_eq!(
        call(&sub, "toolu_01XeTmLfyUTxt5797EY8mD2v")
            .subagent_id
            .as_deref(),
        Some(NESTED)
    );
    assert_eq!(
        sub.subagents
            .iter()
            .map(|r| r.agent_id.as_str())
            .collect::<Vec<_>>(),
        [NESTED]
    );
    assert!(sub.inherited.is_none() && sub.orphan_subagent_ids.is_empty());
    assert!(sub.hidden_count > 0);

    // toolUseResult.agentId when the meta has no toolUseId
    let mut sidecar = s.sidecar.clone();
    for m in &mut sidecar.subagent_metas {
        if m.agent_id == SYNC {
            m.meta_json = Some(r#"{"agentType":"codex:codex-rescue"}"#.into());
        }
    }
    let project = fixture_dir("subagents")
        .join("projects")
        .join(ids::SUBAGENTS_DIR);
    let main = load(
        &project.join(format!("{}.jsonl", ids::SUBAGENTS)),
        FileRole::Main,
        None,
    );
    let s2 = assemble(ids::SUBAGENTS, &s.session_dir, vec![main], sidecar);
    assert_eq!(
        s2.subagent_by_tool
            .get("toolu_011zFxQKA9ZqiQjNSrpVfbB7")
            .map(String::as_str),
        Some(SYNC)
    );
    assert_eq!(s2.orphan_subagent_ids, [ORPHAN]);
}

#[test]
fn background_subagent_without_notification_has_no_result() {
    // The run is still going: its notification is not in the main file yet.
    let project = fixture_dir("subagents")
        .join("projects")
        .join(ids::SUBAGENTS_DIR);
    let mut main = load(
        &project.join(format!("{}.jsonl", ids::SUBAGENTS)),
        FileRole::Main,
        None,
    );
    let before = main.records.len();
    main.records.retain(|r| {
        r.entry
            .content_text()
            .is_none_or(|t| !t.trim_start().starts_with("<task-notification>"))
    });
    assert_eq!(main.records.len(), before - 1);
    let session_dir = project.join(ids::SUBAGENTS);
    let sidecar = load_sidecar(&session_dir);
    let s = assemble(ids::SUBAGENTS, &session_dir, vec![main], sidecar);
    let t = main_t(&s, false);
    let run = t.subagents.iter().find(|r| r.agent_id == ASYNC).unwrap();
    assert!(run.is_async);
    assert_eq!(run.status.as_deref(), Some("async_launched"));
    assert_eq!(run.final_text, None);
}

#[test]
fn teammate_messages_are_not_human_prompts_and_empty_system_entries_get_text() {
    let s = open("subagents", ids::SUBAGENTS_DIR, ids::SUBAGENTS);
    let t = main_t(&s, false);
    let n = t
        .nodes
        .iter()
        .find(|n| n.id == "7e1a0c55-3b2d-5f4e-9a61-0d2c8b7f4e10")
        .expect("teammate message on the Main Line");
    match &n.body {
        NodeBody::UserPrompt { origin, text, .. } => {
            assert_eq!(
                origin,
                &PromptOrigin::Teammate {
                    teammate_id: Some("gui-impl".into()),
                    color: Some("red".into()),
                    summary: Some("GUI shell done, all checks green".into()),
                }
            );
            assert!(text.contains("The GUI shell builds"));
        }
        b => panic!("unexpected {b:?}"),
    }
    // Not a branch head, not counted, not the title.
    assert!(t.branch_points.is_empty());
    let a = summarize(&s.skeleton);
    assert_eq!(a.message_count, 5, "1 prompt + 4 assistant messages");

    // `agents_killed` carries only its envelope: shown with a readable text, never empty.
    let killed = t
        .nodes
        .iter()
        .find(|n| n.id == "a3c9e2f1-6b4d-5e8a-8f20-4d1b7c9e0a36")
        .expect("agents_killed is visible");
    match &killed.body {
        NodeBody::System { subtype, text, .. } => {
            assert_eq!(subtype, "agents_killed");
            assert_eq!(text, "已终止所有后台 agent");
        }
        b => panic!("unexpected {b:?}"),
    }
}

#[test]
fn subagent_scope_assembles_the_agent_file() {
    let s = open("subagents", ids::SUBAGENTS_DIR, ids::SUBAGENTS);
    let f = agent_file(&s, ASYNC);
    let t = transcript(
        &s,
        &request(
            TranscriptScope::Subagent {
                agent_id: ASYNC.into(),
            },
            false,
        ),
        Some(&f),
    )
    .unwrap();
    assert!(matches!(
        &t.nodes[0].body,
        NodeBody::UserPrompt {
            origin: PromptOrigin::Human,
            ..
        }
    ));
    // One merged message spanning tool results: text + Read + Bash + Bash, all paired.
    let calls = tool_calls(&t);
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c.result.is_some()));
    let detail = content::tool_detail(
        &s,
        &ToolDetailRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Subagent {
                agent_id: ASYNC.into(),
            },
            tool_use_id: "toolu_01UjfeU1Mu6ZdYG4nJP76VZo".into(),
            part: ToolPart::Output,
        },
        Some(&f),
    )
    .unwrap();
    assert_eq!(detail.source, DetailSource::Inline);
    assert!(!detail.text.is_empty());
    assert!(
        transcript(
            &s,
            &request(
                TranscriptScope::Subagent {
                    agent_id: ASYNC.into()
                },
                false
            ),
            None
        )
        .is_err()
    );
}

// ---- workflow ----

#[test]
fn workflow_run_from_its_own_files() {
    let session_dir = fixture_dir("workflow")
        .join("projects")
        .join(ids::WORKFLOW_DIR)
        .join(ids::WORKFLOW);
    let sidecar = load_sidecar(&session_dir);
    let files = |run: &str| sidecar.workflows.iter().find(|w| w.run_id == run).unwrap();
    let done = codeck_core::assemble::workflow::run_from_files(files("wf_6e7e94b2-011"));
    assert_eq!(done.status.as_deref(), Some("completed"));
    assert_eq!(done.phases.len(), 2);
    assert_eq!(done.agents[0].label.as_deref(), Some("core-loop"));
    assert_eq!(done.tool_use_id, None);
    // Only a journal: still a run, from the journal.
    let running = codeck_core::assemble::workflow::run_from_files(files("wf_d83f7287-279"));
    assert_eq!(running.status.as_deref(), Some("running"));
    assert!(!running.agents.is_empty());
}

#[test]
fn workflow_runs_from_json_and_from_journal() {
    let s = open("workflow", ids::WORKFLOW_DIR, ids::WORKFLOW);
    let t = main_t(&s, false);
    assert_eq!(t.workflows.len(), 2);
    let done = t
        .workflows
        .iter()
        .find(|w| w.run_id == "wf_6e7e94b2-011")
        .unwrap();
    assert_eq!(done.name.as_deref(), Some("learning-path"));
    assert_eq!(done.status.as_deref(), Some("completed"));
    assert_eq!(
        done.tool_use_id.as_deref(),
        Some("toolu_01Xzqt7LEc8cjvTbYH4Fu9EM")
    );
    assert_eq!(
        done.phases
            .iter()
            .map(|p| (p.index, p.title.as_str()))
            .collect::<Vec<_>>(),
        [(1, "Explore"), (2, "Synthesize")]
    );
    assert_eq!(done.phases[0].detail.as_deref(), Some("parallel readers"));
    assert_eq!(
        done.agents
            .iter()
            .map(|a| a.label.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["core-loop", "model-layer", "tools"]
    );
    assert_eq!(done.agents[0].agent_id, "a3e22cdf3b6a56db2");
    assert_eq!(done.agents[0].phase_index, Some(1));
    assert_eq!(done.agents[0].tool_calls, 16);
    assert_eq!(done.total_tokens, Some(774_097.0));
    assert_eq!(
        call(&t, "toolu_01Xzqt7LEc8cjvTbYH4Fu9EM")
            .workflow_run_id
            .as_deref(),
        Some("wf_6e7e94b2-011")
    );

    let running = t
        .workflows
        .iter()
        .find(|w| w.run_id == "wf_d83f7287-279")
        .unwrap();
    assert_eq!(running.status.as_deref(), Some("running"));
    assert_eq!(
        running.tool_use_id.as_deref(),
        Some("toolu_01TS1vzsEz9yK99oWTsgCgZS")
    );
    let states: HashMap<&str, &str> = running
        .agents
        .iter()
        .map(|a| (a.agent_id.as_str(), a.state.as_deref().unwrap()))
        .collect();
    assert_eq!(states.len(), 2);
    assert_eq!(states["a0423241c80dddf06"], "done");
    assert_eq!(states["a2a3dee6c7ae88510"], "running");

    // Workflow agents belong to their run, not the orphan list.
    assert!(t.orphan_subagent_ids.is_empty());
    assert_eq!(
        t.subagents
            .iter()
            .filter(|r| r.workflow_run_id.is_some())
            .count(),
        5
    );
}

// ---- persisted output and images ----

fn detail(s: &AssembledSession, id: &str, part: ToolPart) -> codeck_core::model::ToolDetail {
    content::tool_detail(
        s,
        &ToolDetailRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Main,
            tool_use_id: id.into(),
            part,
        },
        None,
    )
    .unwrap()
}

#[test]
fn persisted_output_resolves_by_basename() {
    let s = open("persisted_and_images", ids::PERSISTED_DIR, ids::PERSISTED);
    let t = main_t(&s, false);
    let first = call(&t, "toolu_01YSyvoRkw4Y1Uz3WMi4zHxr")
        .result
        .as_ref()
        .unwrap();
    let p = first.persisted.as_ref().unwrap();
    assert_eq!(p.file_name, "b8e6n2j5e.txt");
    assert_eq!(p.size_bytes, 43375.0);
    let d = detail(&s, "toolu_01YSyvoRkw4Y1Uz3WMi4zHxr", ToolPart::Output);
    assert_eq!(d.source, DetailSource::PersistedFile);
    let on_disk = fs::read_to_string(s.session_dir.join("tool-results/b8e6n2j5e.txt")).unwrap();
    assert_eq!(d.text, on_disk);
    assert_eq!(d.total_bytes, on_disk.len() as f64);

    let missing = call(&t, "toolu_ca167d4941e85a55a11e26b4")
        .result
        .as_ref()
        .unwrap();
    assert_eq!(
        missing.persisted.as_ref().unwrap().file_name,
        "zz9missing1.txt"
    );
    let d = detail(&s, "toolu_ca167d4941e85a55a11e26b4", ToolPart::Output);
    assert_eq!(d.source, DetailSource::Missing);
    assert!(d.text.starts_with("<persisted-output>"));

    let input = detail(&s, "toolu_01YSyvoRkw4Y1Uz3WMi4zHxr", ToolPart::Input);
    assert_eq!(input.source, DetailSource::Inline);
    assert!(
        serde_json::from_str::<serde_json::Value>(&input.text)
            .unwrap()
            .is_object()
    );
    assert!(
        content::tool_detail(
            &s,
            &ToolDetailRequest {
                session_id: "s".into(),
                scope: TranscriptScope::Main,
                tool_use_id: "nope".into(),
                part: ToolPart::Input,
            },
            None
        )
        .is_err()
    );
}

#[test]
fn persisted_output_never_leaves_tool_results() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path().join("sid");
    fs::create_dir_all(session_dir.join("tool-results")).unwrap();
    fs::write(session_dir.join("secret.txt"), "outside tool-results").unwrap();
    fs::write(tmp.path().join("secret.txt"), "outside the session").unwrap();
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"x"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"y"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","toolUseResult":{"persistedOutputPath":"../secret.txt","persistedOutputSize":20},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"<persisted-output>\nOutput too large. Full output saved to: ../secret.txt\n\nPreview:\nabc\n</persisted-output>"}]}}"#,
        r#"{"type":"user","uuid":"r2","parentUuid":"r1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"<persisted-output>\nFull output saved to: /x/tool-results/..\n</persisted-output>"}]}}"#,
    ]);
    let s = assemble("sid", &session_dir, vec![f], SessionSidecar::default());
    let t = main_t(&s, false);
    // `../secret.txt` → basename `secret.txt`, looked up only under tool-results/ (absent).
    assert_eq!(
        call(&t, "t1")
            .result
            .as_ref()
            .unwrap()
            .persisted
            .as_ref()
            .unwrap()
            .file_name,
        "secret.txt"
    );
    let d = detail(&s, "t1", ToolPart::Output);
    assert_eq!(d.source, DetailSource::Missing);
    assert!(!d.text.contains("outside"));
    // A `..` basename is rejected outright.
    assert!(call(&t, "t2").result.as_ref().unwrap().persisted.is_none());
    assert_eq!(
        detail(&s, "t2", ToolPart::Output).source,
        DetailSource::Inline
    );
    assert!(content::persisted_path(&session_dir, "../secret.txt").is_none());
}

fn find_tool_output(s: &AssembledSession, choices: Vec<BranchChoice>, query: &str) -> u32 {
    let r = find::find(
        s,
        &FindRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Main,
            branch_choices: choices,
            include_hidden: false,
            query: query.into(),
        },
        None,
    )
    .unwrap();
    r.matches
        .iter()
        .filter(|m| matches!(m.location, FindLocation::ToolOutput { .. }))
        .map(|m| m.count)
        .sum()
}

#[test]
fn card_and_find_take_the_result_on_the_selected_branch() {
    // The same tool_use id on two Branches (rewind to the root prompt), each with its own result.
    let f = from_lines(&[
        r#"{"type":"user","uuid":"p1","parentUuid":null,"message":{"role":"user","content":"one"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"p1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"x"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"alpha out"}]}}"#,
        r#"{"type":"user","uuid":"p2","parentUuid":null,"message":{"role":"user","content":"two"}}"#,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"p2","message":{"id":"m2","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"x"}}]}}"#,
        r#"{"type":"user","uuid":"r2","parentUuid":"a2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"beta out"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let choose = |head: &str| {
        vec![BranchChoice {
            anchor_key: "root".into(),
            head_id: head.into(),
        }]
    };
    for (choices, shown, other) in [
        (Vec::new(), "beta out", "alpha"),
        (choose("p1"), "alpha out", "beta"),
        (choose("p2"), "beta out", "alpha"),
    ] {
        let t = transcript(
            &s,
            &TranscriptRequest {
                session_id: "s".into(),
                scope: TranscriptScope::Main,
                branch_choices: choices.clone(),
                include_hidden: false,
            },
            None,
        )
        .unwrap();
        assert_eq!(call(&t, "t1").result.as_ref().unwrap().text, shown);
        assert_eq!(find_tool_output(&s, choices.clone(), shown), 1);
        assert_eq!(find_tool_output(&s, choices, other), 0);
    }
}

#[test]
fn persisted_output_symlinks_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path().join("sid");
    fs::create_dir_all(session_dir.join("tool-results")).unwrap();
    fs::write(tmp.path().join("secret.txt"), "outside the session").unwrap();
    std::os::unix::fs::symlink(
        tmp.path().join("secret.txt"),
        session_dir.join("tool-results/link.txt"),
    )
    .unwrap();
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"x"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","toolUseResult":{"persistedOutputPath":"/x/tool-results/link.txt","persistedOutputSize":19},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"<persisted-output>\nFull output saved to: /x/tool-results/link.txt\n\nPreview:\nabc\n</persisted-output>"}]}}"#,
    ]);
    let s = assemble("sid", &session_dir, vec![f], SessionSidecar::default());
    let d = detail(&s, "t1", ToolPart::Output);
    assert_eq!(d.source, DetailSource::Missing);
    assert!(!d.text.contains("outside"));
    assert_eq!(find_tool_output(&s, Vec::new(), "outside"), 0);
    assert_eq!(find_tool_output(&s, Vec::new(), "preview"), 1);
}

#[test]
fn images_are_references_fetched_on_demand() {
    let s = open("persisted_and_images", ids::PERSISTED_DIR, ids::PERSISTED);
    let t = main_t(&s, false);
    let read = call(&t, "toolu_013SLWA4qDczfnQSbMHBrbFf")
        .result
        .as_ref()
        .unwrap();
    assert_eq!(read.images.len(), 1);
    let img = &read.images[0];
    assert_eq!(img.media_type, "image/png");
    assert_eq!(
        img.tool_use_id.as_deref(),
        Some("toolu_013SLWA4qDczfnQSbMHBrbFf")
    );
    assert!(img.bytes > 0.0);
    let prompt_images = t
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::UserPrompt { images, .. } if !images.is_empty() => Some(images.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(prompt_images.len(), 1);
    assert!(prompt_images[0].node_id.starts_with("5f4523b2"));
    for image in [img.clone(), prompt_images[0].clone()] {
        let data = content::image(
            &s,
            &ImageRequest {
                session_id: "s".into(),
                scope: TranscriptScope::Main,
                image,
            },
            None,
        )
        .unwrap();
        assert_eq!(data.media_type, "image/png");
        assert!(data.data_base64.starts_with("iVBORw0KGgo"));
    }
    // No base64 payload leaks into the transcript.
    assert!(!serde_json::to_string(&t).unwrap().contains("iVBORw0KGgo"));
}

fn meta(agent_id: &str, json: &str) -> SubagentMetaFile {
    SubagentMetaFile {
        agent_id: agent_id.into(),
        role: FileRole::Subagent,
        workflow_run_id: None,
        transcript_path: None,
        meta_json: Some(json.into()),
    }
}

#[test]
fn subagent_runs_say_how_far_they_got() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t_ok","name":"Agent","input":{"prompt":"p"}},{"type":"tool_use","id":"t_err","name":"Agent","input":{"prompt":"p"}},{"type":"tool_use","id":"t_bg","name":"Agent","input":{"prompt":"p"}},{"type":"tool_use","id":"t_run","name":"Agent","input":{"prompt":"p"}}]}}"#,
        // Foreground: a result without a status, and an error result (a plain string).
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","toolUseResult":{"agentId":"ag_ok"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_ok","content":"all good"}]}}"#,
        r#"{"type":"user","uuid":"r2","parentUuid":"r1","toolUseResult":"Error: interrupted","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_err","is_error":true,"content":"interrupted"}]}}"#,
        // Background: launched, then notified with only the task id.
        r#"{"type":"user","uuid":"r3","parentUuid":"r2","toolUseResult":{"status":"async_launched","agentId":"ag_bg"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_bg","content":"Async agent launched successfully. agentId: ag_bg"}]}}"#,
        r#"{"type":"user","uuid":"r4","parentUuid":"r3","toolUseResult":{"status":"async_launched","agentId":"ag_run"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_run","content":"Async agent launched successfully. agentId: ag_run"}]}}"#,
        r#"{"type":"user","uuid":"n1","parentUuid":"r4","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>\n<task-id>ag_bg</task-id>\n<status>completed</status>\n<summary>Agent \"bg\" finished</summary>\n<result>found 2 bugs &amp; fixed</result>\n</task-notification>"}}"#,
    ]);
    let sidecar = SessionSidecar {
        subagent_metas: vec![
            meta("ag_ok", r#"{"toolUseId":"t_ok"}"#),
            meta("ag_err", r#"{"toolUseId":"t_err"}"#),
            meta("ag_bg", r#"{"toolUseId":"t_bg"}"#),
            meta("ag_run", r#"{"toolUseId":"t_run"}"#),
        ],
        ..SessionSidecar::default()
    };
    let s = assemble("sid", Path::new("/nonexistent"), vec![f], sidecar);
    let t = main_t(&s, false);
    let run = |id: &str| t.subagents.iter().find(|r| r.agent_id == id).unwrap();
    assert_eq!(run("ag_ok").status.as_deref(), Some("completed"));
    assert_eq!(run("ag_ok").final_text.as_deref(), Some("all good"));
    assert_eq!(run("ag_err").status.as_deref(), Some("failed"));
    assert_eq!(run("ag_bg").status.as_deref(), Some("completed"));
    assert_eq!(
        run("ag_bg").final_text.as_deref(),
        Some("found 2 bugs & fixed")
    );
    assert_eq!(call(&t, "t_bg").notification_node_id.as_deref(), Some("n1"));
    assert_eq!(run("ag_run").status.as_deref(), Some("async_launched"));
    assert_eq!(run("ag_run").final_text, None);
}

#[test]
fn nested_background_runs_are_read_from_their_parent_file() {
    let main = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t_p","name":"Agent","input":{"prompt":"p"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","toolUseResult":{"status":"completed","agentId":"ag_p"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_p","content":"done"}]}}"#,
    ]);
    let mut parent = from_lines(&[
        r#"{"type":"user","uuid":"p0","parentUuid":null,"isSidechain":true,"message":{"role":"user","content":"p"}}"#,
        r#"{"type":"assistant","uuid":"p1","parentUuid":"p0","isSidechain":true,"message":{"id":"pm1","content":[{"type":"tool_use","id":"t_n","name":"Agent","input":{"prompt":"q","run_in_background":true}}]}}"#,
        r#"{"type":"user","uuid":"p2","parentUuid":"p1","isSidechain":true,"toolUseResult":{"status":"async_launched","agentId":"ag_n"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_n","content":"Async agent launched successfully. agentId: ag_n"}]}}"#,
    ]);
    parent.role = FileRole::Subagent;
    parent.agent_id = Some("ag_p".into());
    let sidecar = SessionSidecar {
        subagent_metas: vec![
            meta("ag_p", r#"{"toolUseId":"t_p"}"#),
            meta(
                "ag_n",
                r#"{"toolUseId":"t_n","parentAgentId":"ag_p","spawnDepth":1}"#,
            ),
        ],
        ..SessionSidecar::default()
    };
    let s = assemble("sid", Path::new("/nonexistent"), vec![main], sidecar);
    let scope = TranscriptScope::Subagent {
        agent_id: "ag_p".into(),
    };
    let t = transcript(&s, &request(scope, false), Some(&parent)).unwrap();
    let nested = t.subagents.iter().find(|r| r.agent_id == "ag_n").unwrap();
    assert!(nested.is_async);
    assert_eq!(nested.status.as_deref(), Some("async_launched"));
    assert_eq!(nested.final_text, None);
}

#[test]
fn coordinator_messages_show_in_the_subagent_scope() {
    // SendMessage to a background agent: recorded in its file as meta, wrapped by Claude Code.
    let main = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t_p","name":"Agent","input":{"prompt":"review"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","toolUseResult":{"status":"async_launched","agentId":"ag_p"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t_p","content":"Async agent launched successfully. agentId: ag_p"}]}}"#,
    ]);
    let mut agent = from_lines(&[
        r#"{"type":"user","uuid":"p0","parentUuid":null,"isSidechain":true,"message":{"role":"user","content":"review"}}"#,
        r#"{"type":"assistant","uuid":"p1","parentUuid":"p0","isSidechain":true,"message":{"id":"pm1","content":[{"type":"text","text":"2 findings"}]}}"#,
        r#"{"type":"user","uuid":"p2","parentUuid":"p1","isSidechain":true,"isMeta":true,"origin":{"kind":"coordinator"},"message":{"role":"user","content":"The coordinator sent a message while you were working:\nI fixed both; re-review.\n\nAddress this before completing your current task."}}"#,
        r#"{"type":"assistant","uuid":"p3","parentUuid":"p2","isSidechain":true,"message":{"id":"pm2","content":[{"type":"text","text":"approved"}]}}"#,
    ]);
    agent.role = FileRole::Subagent;
    agent.agent_id = Some("ag_p".into());
    let sidecar = SessionSidecar {
        subagent_metas: vec![meta("ag_p", r#"{"toolUseId":"t_p"}"#)],
        ..SessionSidecar::default()
    };
    let s = assemble("sid", Path::new("/nonexistent"), vec![main], sidecar);
    let scope = TranscriptScope::Subagent {
        agent_id: "ag_p".into(),
    };
    let t = transcript(&s, &request(scope, false), Some(&agent)).unwrap();
    let node = t.nodes.iter().find(|n| n.id == "p2").expect("shown");
    assert!(!node.hidden);
    match &node.body {
        NodeBody::UserPrompt { origin, text, .. } => {
            assert_eq!(origin, &PromptOrigin::Coordinator);
            assert_eq!(text, "I fixed both; re-review.");
        }
        b => panic!("unexpected {b:?}"),
    }
    // Not the user's: no Branch head, not a counted message.
    assert!(t.branch_points.is_empty());
}

#[test]
fn slash_command_images_show_on_the_command() {
    // The command line is text only; its pasted image comes in the meta entry after it, with the skill text.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    let meta = format!(
        r#"{{"type":"user","uuid":"m1","parentUuid":"u1","isMeta":true,"message":{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}},{{"type":"text","text":"Base directory for this skill: /x"}}]}}}}"#
    );
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>看图 [Image #1]</command-args>"}}"#,
        &meta,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"m1","message":{"id":"x1","content":[{"type":"text","text":"好"}]}}"#,
    ]);
    let s = assemble(
        "sid",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let images = t
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::UserPrompt { images, .. } if n.id == "u1" => Some(images.clone()),
            _ => None,
        })
        .expect("command prompt");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].node_id, "m1");
    let data = content::image(
        &s,
        &ImageRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Main,
            image: images[0].clone(),
        },
        None,
    )
    .unwrap();
    assert!(data.data_base64.starts_with("iVBORw0KGgo"));
}

// ---- real data (read-only, opt-in) ----

/// Assembles every main session under `$CODECK_REAL_ROOT/projects` (read-only) and reports timings.
/// Run with `CODECK_REAL_ROOT=~/.claude cargo test -p codeck-core --release --test assemble -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_root_assembles_every_session() {
    let Ok(root) = std::env::var("CODECK_REAL_ROOT") else {
        eprintln!("CODECK_REAL_ROOT not set; skipping");
        return;
    };
    let projects = Path::new(&root).join("projects");
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    for dir in fs::read_dir(&projects).unwrap().filter_map(|e| e.ok()) {
        let Ok(rd) = fs::read_dir(dir.path()) else {
            continue;
        };
        for f in rd.filter_map(|e| e.ok()) {
            let p = f.path();
            if p.extension().is_some_and(|x| x == "jsonl") {
                files.push((p, f.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    files.sort_by(|a, b| b.1.cmp(&a.1));
    let mut panics = Vec::new();
    let mut timings = Vec::new();
    let coverage = std::sync::Mutex::new(Vec::new());
    let started = std::time::Instant::now();
    for (path, size) in &files {
        let sid = path.file_stem().unwrap().to_string_lossy().to_string();
        let session_dir = path.with_extension("");
        let res = std::panic::catch_unwind(|| {
            let t0 = std::time::Instant::now();
            let main = load(path, FileRole::Main, None);
            let parsed = t0.elapsed();
            let sidecar = load_sidecar(&session_dir);
            let t1 = std::time::Instant::now();
            let s = assemble(&sid, &session_dir, vec![main], sidecar);
            let assembled = t1.elapsed();
            let t2 = std::time::Instant::now();
            let t = main_t(&s, false);
            let json = serde_json::to_string(&t).unwrap();
            let elapsed = t2.elapsed();
            let total = s.skeleton.nodes.len();
            let on_main = codeck_core::assemble::tree::main_set(&s.skeleton).len();
            coverage
                .lock()
                .unwrap()
                .push((path.clone(), total, on_main));
            (parsed, assembled, elapsed, t.nodes.len(), json.len())
        });
        match res {
            Ok(r) => timings.push((path.clone(), *size, r)),
            Err(_) => panics.push(path.clone()),
        }
    }
    eprintln!(
        "sessions: {}, panics: {}, total: {:?}",
        files.len(),
        panics.len(),
        started.elapsed()
    );
    for p in &panics {
        eprintln!("PANIC: {}", p.display());
    }
    for (path, size, (parsed, assembled, tr, nodes, json)) in timings.iter().take(3) {
        eprintln!(
            "{} ({:.1} MB): parse {:?}, assemble {:?}, transcript {:?}, {} nodes, {:.2} MB json",
            path.display(),
            *size as f64 / 1e6,
            parsed,
            assembled,
            tr,
            nodes,
            *json as f64 / 1e6
        );
    }
    let mut slowest: Vec<_> = timings.iter().collect();
    slowest.sort_by_key(|(_, _, (_, a, t, _, _))| std::cmp::Reverse(*a + *t));
    for (path, size, (_, assembled, tr, _, _)) in slowest.iter().take(3) {
        eprintln!(
            "slowest: {} ({:.1} MB): assemble {:?}, transcript {:?}",
            path.display(),
            *size as f64 / 1e6,
            assembled,
            tr
        );
    }
    // Main Line coverage: a low share of entries in the Main Line display set hints at a broken chain
    // (or genuinely abandoned branches); listed for manual review.
    let mut cov = coverage.into_inner().unwrap();
    cov.retain(|(_, total, _)| *total >= 200);
    cov.sort_by(|a, b| (a.2 as f64 / a.1 as f64).total_cmp(&(b.2 as f64 / b.1 as f64)));
    for (path, total, on_main) in cov.iter().take(5) {
        eprintln!("coverage: {on_main}/{total} {}", path.display());
    }
    assert!(panics.is_empty());
}

#[test]
fn input_queued_while_claude_was_busy_shows_as_user_messages() {
    let queued = |uuid: &str, parent: &str, attachment: &str| {
        format!(
            r#"{{"type":"attachment","uuid":"{uuid}","parentUuid":"{parent}","timestamp":"2026-10-15T04:01:10.111Z","attachment":{attachment}}}"#
        )
    };
    let typed = queued(
        "q1",
        "r1",
        r#"{"type":"queued_command","prompt":"给截图带上标注","commandMode":"prompt","origin":{"kind":"human"},"humanTurn":true}"#,
    );
    // Older versions: no `origin`; content blocks with an image.
    let with_image = queued(
        "q2",
        "q1",
        r#"{"type":"queued_command","prompt":[{"type":"text","text":"[Image #1]\n还有这种表现"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}],"commandMode":"prompt"}"#,
    );
    let notification = queued(
        "q3",
        "q2",
        r#"{"type":"queued_command","prompt":"<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>t9</tool-use-id>\n<status>completed</status>\n</task-notification>","commandMode":"task-notification"}"#,
    );
    let peer = queued(
        "q4",
        "q3",
        r#"{"type":"queued_command","prompt":"<agent-message from=\"impl\">done</agent-message>","commandMode":"prompt","origin":{"kind":"peer"}}"#,
    );
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"截几张图"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"x"}}]}}"#,
        r#"{"type":"user","uuid":"r1","parentUuid":"a1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
        &typed,
        &with_image,
        &notification,
        &peer,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"q4","message":{"id":"m2","content":[{"type":"text","text":"好的"}]}}"#,
    ]);
    let s = assemble(
        "sid",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let prompt = |id: &str| match &t
        .nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("{id} not shown"))
        .body
    {
        NodeBody::UserPrompt {
            text,
            images,
            origin,
        } => (text.clone(), images.len(), origin.clone()),
        b => panic!("{id}: unexpected {b:?}"),
    };
    assert_eq!(
        prompt("q1"),
        ("给截图带上标注".into(), 0, PromptOrigin::Human)
    );
    let (text, images, origin) = prompt("q2");
    assert!(text.contains("还有这种表现"));
    assert_eq!((images, origin), (1, PromptOrigin::Human));
    assert!(matches!(
        prompt("q3").2,
        PromptOrigin::TaskNotification { .. }
    ));
    assert!(t.nodes.iter().any(|n| n.id == "q4"));

    // The two typed prompts count as the user's messages; the notification and the peer don't.
    let a = summarize(&s.skeleton);
    assert_eq!(a.message_count, 5, "3 human prompts + 2 assistant messages");
}

#[test]
fn local_commands_recorded_as_system_entries_show_as_commands() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"看下模板"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"text","text":"好的"}]}}"#,
        r#"{"type":"system","subtype":"local_command","uuid":"c1","parentUuid":"a1","level":"info","content":"<command-name>/rename</command-name>\n            <command-message>rename</command-message>\n            <command-args>wmc template issue</command-args>"}"#,
        r#"{"type":"system","subtype":"local_command","uuid":"o1","parentUuid":"c1","level":"info","content":"<local-command-stdout>Session renamed to: wmc template issue</local-command-stdout>"}"#,
        r#"{"type":"system","subtype":"local_command","uuid":"c2","parentUuid":"o1","level":"info","content":"<command-name>/mcp</command-name>\n            <command-message>mcp</command-message>\n            <command-args></command-args>"}"#,
        r#"{"type":"system","subtype":"local_command","uuid":"o2","parentUuid":"c2","level":"info","content":"<local-command-stdout></local-command-stdout>"}"#,
    ]);
    let s = assemble(
        "sid",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let origin = |id: &str| match &t.nodes.iter().find(|n| n.id == id).map(|n| &n.body) {
        Some(NodeBody::UserPrompt { origin, .. }) => Some(origin.clone()),
        Some(b) => panic!("{id}: unexpected {b:?}"),
        None => None,
    };
    assert_eq!(
        origin("c1"),
        Some(PromptOrigin::Command {
            name: "/rename".into(),
            args: "wmc template issue".into()
        })
    );
    assert_eq!(origin("o1"), Some(PromptOrigin::CommandOutput));
    // An empty output is hidden.
    assert_eq!(origin("o2"), None);
    assert_eq!(t.hidden_count, 1);

    let a = summarize(&s.skeleton);
    assert_eq!(
        a.message_count, 4,
        "1 prompt + 2 commands + 1 assistant message"
    );
}
