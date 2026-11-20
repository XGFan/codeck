//! Workflow Runs: their state from their own files, reported when it changes.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use ::transcript::assemble::{WorkflowFiles, workflow};
use ::transcript::layout;
use serde_json::{Value, json};

use super::TxEvent;

/// What was last seen of a Workflow Run's files.
#[derive(Debug, Default)]
struct WorkflowSeen {
    sig: Vec<(u64, Option<SystemTime>)>,
    tool_use_id: Option<String>,
    last: Option<Value>,
}

#[derive(Debug, Default)]
pub(super) struct Workflows {
    /// Workflow Run id → its `Workflow` tool_use id (from the tool result's `runId`).
    run_tools: HashMap<String, String>,
    seen: HashMap<String, WorkflowSeen>,
}

impl Workflows {
    /// A `Workflow` tool result named its run.
    pub(super) fn note_run(&mut self, run_id: String, tool_use_id: String) {
        self.run_tools.entry(run_id).or_insert(tool_use_id);
    }

    /// The `Workflow` tool call behind `run`, once its result was read.
    pub(super) fn tool_of(&self, run: &str) -> Option<String> {
        self.run_tools.get(run).cloned()
    }

    /// Reports every Workflow Run again on the next look (its current state).
    pub(super) fn forget(&mut self) {
        self.seen.clear();
    }

    /// Takes over what `old` reported (the main file is read again from its start): nothing
    /// already reported is reported twice.
    pub(super) fn keep_seen(&mut self, old: Workflows) {
        self.seen = old.seen;
    }

    /// Reports every Workflow Run under the Session's directory `dir` whose files or tool call
    /// changed since the last look.
    pub(super) fn poll(&mut self, dir: &Path, out: &mut Vec<TxEvent>) {
        let json_dir = layout::workflows_dir(dir);
        let journal_dir = layout::workflow_agents_dir(dir);
        let mut runs: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&json_dir) {
            for e in rd.flatten() {
                if let Some(r) = e.file_name().to_str().and_then(layout::workflow_json_run) {
                    runs.push(r.to_string());
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&journal_dir) {
            for e in rd.flatten() {
                if e.path().join(layout::JOURNAL).is_file()
                    && let Some(n) = e.file_name().to_str()
                {
                    runs.push(n.to_string());
                }
            }
        }
        runs.sort();
        runs.dedup();
        for run in runs {
            let json_path = json_dir.join(format!("{run}.json"));
            let journal_path = journal_dir.join(&run).join(layout::JOURNAL);
            let stat = |p: &Path| {
                std::fs::metadata(p)
                    .map(|m| (m.len(), m.modified().ok()))
                    .unwrap_or((0, None))
            };
            let sig = vec![stat(&json_path), stat(&journal_path)];
            let tool = self.run_tools.get(&run).cloned();
            let seen = self.seen.entry(run.clone()).or_default();
            if seen.sig == sig && seen.tool_use_id == tool {
                continue;
            }
            seen.sig = sig;
            seen.tool_use_id = tool.clone();
            let files = WorkflowFiles {
                run_id: run.clone(),
                workflow_json: std::fs::read_to_string(&json_path).ok(),
                journal_jsonl: std::fs::read_to_string(&journal_path).ok(),
            };
            let r = workflow::run_from_files(&files);
            let v = json!({
                "type": "workflow",
                "run_id": r.run_id,
                "tool_use_id": tool,
                "name": r.name,
                "status": r.status,
                "phases": r.phases.iter().map(|p| json!({
                    "index": p.index, "title": p.title, "detail": p.detail,
                })).collect::<Vec<_>>(),
                "agents": r.agents.iter().map(|a| json!({
                    "agent_id": a.agent_id, "label": a.label, "phase_index": a.phase_index,
                    "state": a.state,
                })).collect::<Vec<_>>(),
            });
            if seen.last.as_ref() != Some(&v) {
                seen.last = Some(v.clone());
                out.push(TxEvent::Workflow(v));
            }
        }
    }
}
