//! Shared test helpers for `transcript` integration tests.
#[path = "../../../codeck-core/tests/common/fixtures.rs"]
pub mod fixtures;

use std::fs;
use std::path::{Path, PathBuf};

use fixtures::fixture_dir;
use transcript::assemble::{
    AssembledSession, LoadedFile, SessionSidecar, SubagentMetaFile, WorkflowFiles, assemble,
    transcript,
};
use transcript::model::{
    AssistantBlock, FileRole, NodeBody, ToolCall, Transcript, TranscriptRequest, TranscriptScope,
};
use transcript::parse::{parse_bytes, read_entries};

pub fn load(path: &Path, role: FileRole, agent_id: Option<String>) -> LoadedFile {
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
pub fn load_sidecar(session_dir: &Path) -> SessionSidecar {
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

pub fn open(scenario: &str, dir: &str, sid: &str) -> AssembledSession {
    let project = fixture_dir(scenario).join("projects").join(dir);
    let main = load(&project.join(format!("{sid}.jsonl")), FileRole::Main, None);
    let session_dir = project.join(sid);
    let sidecar = load_sidecar(&session_dir);
    assemble(sid, &session_dir, vec![main], sidecar)
}

pub fn request(scope: TranscriptScope, include_hidden: bool) -> TranscriptRequest {
    TranscriptRequest {
        session_id: "s".into(),
        scope,
        branch_choices: Vec::new(),
        include_hidden,
    }
}

pub fn main_t(s: &AssembledSession, include_hidden: bool) -> Transcript {
    transcript(s, &request(TranscriptScope::Main, include_hidden), None).unwrap()
}

pub fn tool_calls(t: &Transcript) -> Vec<&ToolCall> {
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

pub fn call<'a>(t: &'a Transcript, tool_use_id: &str) -> &'a ToolCall {
    tool_calls(t)
        .into_iter()
        .find(|c| c.tool_use_id == tool_use_id)
        .unwrap_or_else(|| panic!("no tool call {tool_use_id}"))
}

pub fn from_lines(lines: &[&str]) -> LoadedFile {
    let text = lines.join("\n") + "\n";
    LoadedFile {
        path: PathBuf::from("inline.jsonl"),
        role: FileRole::Main,
        agent_id: None,
        records: parse_bytes(text.as_bytes(), 0).records,
    }
}
