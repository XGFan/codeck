//! Background tasks of the main line: started by a tool result, ended by a notification.

use std::collections::HashSet;

use ::transcript::assemble::{prompt, subagent};
use serde::Serialize;
use serde_json::Value;

use super::block::PendingTool;

/// What a background task runs; on the wire `agent`, `shell`, `monitor` or `workflow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Agent,
    Shell,
    /// The Monitor tool's watch, which notifies on each event (Claude Code runs it as a background
    /// shell, and Stop's `background_tasks` calls it `shell`).
    Monitor,
    Workflow,
}

impl TaskKind {
    /// The kind as [`subagent::background_launch`] names it.
    pub fn from_name(name: &str) -> Option<TaskKind> {
        match name {
            "agent" => Some(TaskKind::Agent),
            "shell" => Some(TaskKind::Shell),
            "monitor" => Some(TaskKind::Monitor),
            "workflow" => Some(TaskKind::Workflow),
            _ => None,
        }
    }
}

/// A background task Claude Code started: a Subagent Run, a shell, a Monitor or a Workflow Run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BgTask {
    /// Claude Code's task id (agentId, background shell id, Workflow task id).
    pub id: String,
    pub kind: TaskKind,
    pub tool_use_id: String,
    pub description: Option<String>,
    /// The shell command of a background shell or a Monitor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `running` until Claude Code reports it ended (`completed`, `failed`, `killed`, …).
    pub status: String,
    /// A running Teammate between rounds, waiting for a message. Claude Code keeps it `running` and
    /// leaves it out of its own count of what is working.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub idle: bool,
}

#[derive(Debug, Default)]
pub(super) struct Tasks {
    /// In start order.
    list: Vec<BgTask>,
    /// Tasks whose notifications are a Monitor's ([`monitor_notice`]).
    monitors: HashSet<String>,
}

impl Tasks {
    /// Background tasks seen so far.
    pub(super) fn list(&self) -> &[BgTask] {
        &self.list
    }

    /// Tasks a Monitor's notification named.
    pub(super) fn monitors(&self) -> &HashSet<String> {
        &self.monitors
    }

    /// A task ccrpc stopped: `stopped` until Claude Code's own notice says otherwise.
    pub(super) fn mark_stopped(&mut self, id: &str) {
        if let Some(t) = self
            .list
            .iter_mut()
            .find(|t| t.id == id && t.status == "running")
        {
            t.status = "stopped".into();
        }
    }

    /// A `<task-notification>` of the main line (delivered at once, or queued while busy): a
    /// Monitor's names its task as one; one that ended a task gives it its status.
    pub(super) fn note_notification(&mut self, text: &str) {
        if let Some(id) = monitor_notice(text) {
            self.monitors.insert(id);
        }
        if let Some((id, status)) = task_notification(text)
            && let Some(t) = self.list.iter_mut().find(|t| t.id == id)
        {
            t.status = status;
        }
    }

    /// A main-line tool result (`tool`: its call, when it was seen). A `TaskStop` that worked ends
    /// its task: a task stopped this way gets no notification of its own (a shell's never does),
    /// so its result is the only record that the task ended. A launch in the background starts one.
    pub(super) fn on_tool_result(
        &mut self,
        tool: Option<&PendingTool>,
        tool_use_id: &str,
        text: &str,
        is_error: bool,
    ) {
        if !is_error
            && let Some(t) = tool.filter(|t| t.name == "TaskStop")
            && let Some(id) = ["task_id", "shell_id"]
                .iter()
                .find_map(|k| t.input.get(*k).and_then(Value::as_str))
        {
            let id = id.to_string();
            self.mark_stopped(&id);
        }
        if let Some((kind, id)) = subagent::background_launch(text)
            && let Some(kind) = TaskKind::from_name(kind)
            && !self.list.iter().any(|t| t.id == id)
        {
            let input = tool.map(|t| &t.input);
            let field = |k: &str| {
                input
                    .and_then(|i| i.get(k))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let summary = text
                .lines()
                .find_map(|l| l.strip_prefix("Summary: "))
                .map(str::to_string);
            self.list.push(BgTask {
                id,
                kind,
                tool_use_id: tool_use_id.to_string(),
                description: field("description").or(summary),
                command: field("command"),
                status: "running".into(),
                idle: false,
            });
        }
    }
}

/// The task a Monitor's `<task-notification>` is about: one of its events (`Monitor event: "…"`)
/// or the end of its stream (`Monitor "…" stream ended`).
pub fn monitor_notice(text: &str) -> Option<String> {
    let n = prompt::task_notification(text)?;
    let s = n.summary?;
    (s.starts_with("Monitor event: ")
        || (s.starts_with("Monitor \"") && s.ends_with(" stream ended")))
    .then_some(n.task_id)
    .flatten()
}

/// A `<task-notification>` that ended a task: `(task id, status)`.
pub fn task_notification(text: &str) -> Option<(String, String)> {
    let n = prompt::task_notification(text)?;
    Some((n.task_id?, n.status?))
}
