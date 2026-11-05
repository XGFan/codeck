//! Where a Session's agent and sidecar files live inside its directory (`<project dir>/<sid>/`):
//!
//! - `subagents/agent-<id>.jsonl` (+ `agent-<id>.meta.json`): Subagent Run;
//! - `subagents/workflows/<runId>/agent-<id>.jsonl` (+ `.meta.json`, `journal.jsonl`): Workflow
//!   Run agents;
//! - `workflows/<runId>.json`: Workflow Run summary.
//!
//! Only paths and names: listing, stat and dedupe stay with the callers (codeck-core's scan,
//! ccrpc's follower).

use std::path::{Path, PathBuf};

/// Name of a Workflow Run's journal inside its agents directory.
pub const JOURNAL: &str = "journal.jsonl";

/// `<session dir>/subagents`: Subagent Run files.
pub fn subagents_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("subagents")
}

/// `<session dir>/subagents/workflows`: one directory per Workflow Run, holding its agents.
pub fn workflow_agents_dir(session_dir: &Path) -> PathBuf {
    subagents_dir(session_dir).join("workflows")
}

/// `<session dir>/workflows`: one `<runId>.json` summary per Workflow Run.
pub fn workflows_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("workflows")
}

/// A file name in an agents directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFile<'a> {
    /// `agent-<id>.jsonl`.
    Transcript(&'a str),
    /// `agent-<id>.meta.json`.
    Meta(&'a str),
}

/// Classifies `name` (a file name in an agents directory); `None` for anything else.
pub fn agent_file(name: &str) -> Option<AgentFile<'_>> {
    let rest = name.strip_prefix("agent-")?;
    if let Some(id) = rest.strip_suffix(".meta.json") {
        Some(AgentFile::Meta(id))
    } else {
        rest.strip_suffix(".jsonl").map(AgentFile::Transcript)
    }
}

/// `agent-<id>.meta.json` next to an agent's transcript in `dir`.
pub fn agent_meta_path(dir: &Path, agent_id: &str) -> PathBuf {
    dir.join(format!("agent-{agent_id}.meta.json"))
}

/// Run id of a file name in [`workflows_dir`] (`<runId>.json`).
pub fn workflow_json_run(name: &str) -> Option<&str> {
    name.strip_suffix(".json")
}
