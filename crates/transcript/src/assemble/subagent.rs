//! Subagent meta parsing, linking (toolUseId → agentId → name) and `SubagentRun`.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::nodes::{self, PREVIEW_BYTES, RESULT_BYTES, RESULT_LINES};
use super::{EntryKind, LoadedFile, SessionSidecar, SessionSkeleton, prompt};
use crate::known::AGENT_TOOLS;
use crate::model::{AgentId, NodeId, PromptOrigin, SubagentRun};
use crate::raw::{RawToolUseResult, lenient};

/// Result of linking the Session's Subagent Runs to the main file.
#[derive(Debug, Default)]
pub struct Links {
    pub runs: Vec<SubagentRun>,
    pub orphans: Vec<AgentId>,
    /// Main-file tool_use id → agent id.
    pub by_tool: HashMap<String, AgentId>,
    /// tool_use id → task-notification prompt uuid.
    pub notification_by_tool: HashMap<String, NodeId>,
}

/// `agent-<id>.meta.json`; every field optional, unknown fields ignored.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentMeta {
    #[serde(default, deserialize_with = "lenient::string")]
    pub agent_type: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub tool_use_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub spawn_depth: Option<f64>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub parent_agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub model: Option<String>,
}

pub fn parse_meta(json: Option<&str>) -> AgentMeta {
    json.and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default()
}

/// A tool result saying a background task started: `(kind, task id)` with kind `agent`
/// (Agent / Task with `run_in_background`), `shell` (Bash), `monitor` (Monitor) or `workflow`.
pub fn background_launch(text: &str) -> Option<(&'static str, String)> {
    let after = |mark: &str| {
        let rest = &text[text.find(mark)? + mark.len()..];
        let id: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        (!id.is_empty()).then_some(id)
    };
    if text.starts_with("Async agent launched") {
        return after("agentId:").map(|id| ("agent", id));
    }
    if text.starts_with("Command running in background with ID:") {
        return after("with ID:").map(|id| ("shell", id));
    }
    if text.starts_with("Workflow launched in background") {
        return after("Task ID:").map(|id| ("workflow", id));
    }
    // The Monitor tool: `Monitor started (task b6wmxusge, timeout 900000ms). …`.
    if text.starts_with("Monitor started (task") {
        return after("(task").map(|id| ("monitor", id));
    }
    // A foreground command that hit its timeout keeps running in the background; the notice
    // follows the output it printed so far.
    if text.contains("was moved to the background (ID:") {
        return after("moved to the background (ID:").map(|id| ("shell", id));
    }
    None
}

/// A background task's notifications, merged: the first one's node (where the card links to), the
/// last one's status and result (a resumed agent notifies again when it stops).
#[derive(Debug, Clone)]
pub struct Notification {
    pub node_id: NodeId,
    pub status: Option<String>,
    pub summary: Option<String>,
    /// `<result>`: the agent's final text (capped).
    pub result: Option<String>,
    /// Order of the last notification merged in.
    seq: u32,
}

/// Task-notification prompts by their `<tool-use-id>` and by their `<task-id>` (for an agent,
/// its agent id); some notifications carry only the task id.
#[derive(Debug, Default)]
pub struct Notices {
    pub by_tool: HashMap<String, Notification>,
    pub by_task: HashMap<String, Notification>,
}

impl Notices {
    /// The notifications of the background task launched by `tool_use_id` / with id `task_id`.
    pub fn get(&self, tool_use_id: Option<&str>, task_id: &str) -> Option<&Notification> {
        let by_tool = tool_use_id.and_then(|t| self.by_tool.get(t));
        let by_task = self.by_task.get(task_id);
        match (by_tool, by_task) {
            // The later-indexed entry of the two holds the newer status.
            (Some(a), Some(b)) => Some(if b.seq > a.seq { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

pub fn notification_facts(s: &SessionSkeleton, files: &[LoadedFile]) -> Notices {
    let mut out = Notices::default();
    let mut seq = 0u32;
    for (i, n) in s.nodes.iter().enumerate() {
        if n.entry_type != EntryKind::User || n.is_tool_result || n.is_compact_summary || n.is_human
        {
            continue;
        }
        let e = s.entry(files, i);
        let tagged = e.origin.as_ref().and_then(|o| o.kind.as_deref()) == Some("task-notification")
            || e.content_text()
                .is_some_and(|t| t.trim_start().starts_with("<task-notification>"));
        if !tagged {
            continue;
        }
        let text = prompt::user_text(e);
        let PromptOrigin::TaskNotification {
            task_id,
            tool_use_id,
            status,
            summary,
        } = prompt::classify(e, &text)
        else {
            continue;
        };
        seq += 1;
        let result = prompt::tag(&text, "result")
            .map(|r| nodes::cap_text(&prompt::unescape_xml(&r), RESULT_BYTES, RESULT_LINES).0);
        let fresh = Notification {
            node_id: n.uuid.clone(),
            status,
            summary,
            result,
            seq,
        };
        for (map, key) in [(&mut out.by_tool, tool_use_id), (&mut out.by_task, task_id)] {
            let Some(key) = key else { continue };
            map.entry(key)
                .and_modify(|old| {
                    *old = Notification {
                        node_id: old.node_id.clone(),
                        ..fresh.clone()
                    }
                })
                .or_insert_with(|| fresh.clone());
        }
    }
    out
}

/// Facts from the results of Agent/Task tool calls in one file.
#[derive(Default)]
struct AgentResults {
    /// `toolUseResult.agentId` → tool_use id.
    by_agent: HashMap<String, String>,
    /// `toolUseResult.name` (teammates) → tool_use id.
    by_name: HashMap<String, String>,
    /// tool_use id → `toolUseResult.status`.
    status: HashMap<String, String>,
}

fn agent_results(s: &SessionSkeleton, files: &[LoadedFile]) -> AgentResults {
    let mut out = AgentResults::default();
    for (i, n) in s.nodes.iter().enumerate() {
        let Some(id) = n.tool_results.iter().find(|id| {
            s.tool_use_at.get(*id).is_some_and(|&u| {
                s.nodes[u]
                    .tool_uses
                    .iter()
                    .any(|(tid, name)| tid == *id && AGENT_TOOLS.contains(&name.as_str()))
            })
        }) else {
            continue;
        };
        let Some(r): Option<RawToolUseResult> = s.entry(files, i).tool_use_result_fields() else {
            continue;
        };
        if let Some(a) = r.agent_id {
            out.by_agent.entry(a).or_insert_with(|| id.clone());
        }
        if let Some(name) = r.name {
            out.by_name.entry(name).or_insert_with(|| id.clone());
        }
        if let Some(st) = r.status {
            out.status.insert(id.clone(), st);
        }
    }
    out
}

#[derive(Deserialize, Default)]
struct AgentInput {
    #[serde(default, deserialize_with = "lenient::string")]
    prompt: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    description: Option<String>,
}

fn agent_input(s: &SessionSkeleton, files: &[LoadedFile], tool_use_id: &str) -> AgentInput {
    s.tool_use_block(files, tool_use_id)
        .and_then(|(_, b)| b.input.as_deref())
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .unwrap_or_default()
}

/// The tool_result for `tool_use_id`, if any: its capped text (`None` when empty) and `is_error`.
fn result_of(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    tool_use_id: &str,
) -> Option<(Option<String>, bool)> {
    // Session-wide, not per display path: a run's outcome does not change with branch choices.
    let (_, _, block) = s.tool_result_block(files, tool_use_id, None)?;
    let (text, _) = nodes::result_content(block.content.as_deref());
    let text = text.trim();
    let text = (!text.is_empty()).then(|| nodes::cap_text(text, RESULT_BYTES, RESULT_LINES).0);
    Some((text, block.is_error == Some(true)))
}

/// How far the run `agent_id`, launched by `tool_use_id` in `s`, has got: `(is_async, status, final_text)`.
/// A background run ends with its task-notification (its tool_result only says it launched); a foreground
/// one with its tool_result, which may carry no status (an error result is a plain string).
fn outcome(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    results: &AgentResults,
    notes: &Notices,
    tool_use_id: &str,
    agent_id: &str,
) -> (bool, Option<String>, Option<String>) {
    let result_status = results.status.get(tool_use_id).cloned();
    let is_async = result_status.as_deref() == Some("async_launched");
    let note = notes.get(Some(tool_use_id), agent_id);
    let note_status = note.and_then(|n| n.status.clone());
    if is_async {
        let text = note.and_then(|n| n.result.clone().or_else(|| n.summary.clone()));
        return (true, note_status.or(result_status), text);
    }
    let result = result_of(s, files, tool_use_id);
    let ended = result
        .as_ref()
        .map(|(_, err)| if *err { "failed" } else { "completed" }.to_owned());
    let status = note_status.or(result_status).or(ended);
    let text = result
        .and_then(|(t, _)| t)
        .or_else(|| note.and_then(|n| n.summary.clone()));
    (false, status, text)
}

/// Inside an agent file (Subagent scope): fills in how its nested runs went (the Session-wide runs
/// only know their meta), and returns tool_use id → task-notification uuid for this file.
pub fn nested_outcomes(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    nested: &mut [SubagentRun],
    by_tool: &HashMap<String, AgentId>,
) -> HashMap<String, NodeId> {
    let results = agent_results(s, files);
    let notes = notification_facts(s, files);
    let mut links: HashMap<String, NodeId> = notes
        .by_tool
        .iter()
        .map(|(k, v)| (k.clone(), v.node_id.clone()))
        .collect();
    for (tool, agent) in by_tool {
        let Some(run) = nested.iter_mut().find(|r| &r.agent_id == agent) else {
            continue;
        };
        (run.is_async, run.status, run.final_text) =
            outcome(s, files, &results, &notes, tool, agent);
        if let Some(n) = notes.by_task.get(agent) {
            links
                .entry(tool.clone())
                .or_insert_with(|| n.node_id.clone());
        }
    }
    links
}

/// Links every sidecar Subagent Run: `meta.toolUseId` in the main file, a nested run's
/// `toolUseId` (resolved in its parent's file via `parentAgentId`), `toolUseResult.agentId`,
/// then `meta.name == toolUseResult.name`. Workflow agents belong to their Workflow Run.
pub fn link(s: &SessionSkeleton, files: &[LoadedFile], sidecar: &SessionSidecar) -> Links {
    let results = agent_results(s, files);
    let notes = notification_facts(s, files);
    let known: HashSet<&str> = sidecar
        .subagent_metas
        .iter()
        .map(|m| m.agent_id.as_str())
        .collect();
    let mut links = Links {
        notification_by_tool: notes
            .by_tool
            .iter()
            .map(|(k, v)| (k.clone(), v.node_id.clone()))
            .collect(),
        ..Links::default()
    };
    for m in &sidecar.subagent_metas {
        let meta = parse_meta(m.meta_json.as_deref());
        let in_main = |t: &String| s.tool_use_at.contains_key(t);
        let main_tool = meta
            .tool_use_id
            .clone()
            .filter(in_main)
            .filter(|_| m.workflow_run_id.is_none());
        let nested = main_tool.is_none()
            && m.workflow_run_id.is_none()
            && meta.tool_use_id.is_some()
            && meta
                .parent_agent_id
                .as_deref()
                .is_some_and(|p| known.contains(p));
        let main_tool = if nested || m.workflow_run_id.is_some() {
            None
        } else {
            main_tool
                .or_else(|| results.by_agent.get(&m.agent_id).cloned())
                .or_else(|| {
                    meta.name
                        .as_ref()
                        .and_then(|n| results.by_name.get(n).cloned())
                })
        };
        let linked = main_tool.is_some() || nested || m.workflow_run_id.is_some();
        if !linked {
            links.orphans.push(m.agent_id.clone());
        }
        if let Some(t) = &main_tool {
            links
                .by_tool
                .entry(t.clone())
                .or_insert_with(|| m.agent_id.clone());
        }
        let tool_use_id = main_tool
            .clone()
            .or_else(|| meta.tool_use_id.clone().filter(|_| nested));
        let (is_async, status, final_text) = match main_tool.as_deref() {
            Some(t) => outcome(s, files, &results, &notes, t, &m.agent_id),
            None => (false, None, None),
        };
        if let (Some(t), Some(n)) = (&main_tool, notes.by_task.get(&m.agent_id)) {
            // A notification carrying only the task id still links to the card.
            links
                .notification_by_tool
                .entry(t.clone())
                .or_insert_with(|| n.node_id.clone());
        }
        let input = main_tool
            .as_deref()
            .map(|t| agent_input(s, files, t))
            .unwrap_or_default();
        let stats = sidecar
            .agent_stats
            .get(&m.agent_id)
            .cloned()
            .unwrap_or_default();
        links.runs.push(SubagentRun {
            agent_id: m.agent_id.clone(),
            agent_type: meta.agent_type.clone(),
            description: meta.description.clone().or(input.description),
            name: meta.name.clone(),
            parent_agent_id: meta.parent_agent_id.clone(),
            spawn_depth: meta.spawn_depth.map(|d| d.max(0.0) as u32).unwrap_or(0),
            tool_use_id,
            workflow_run_id: m.workflow_run_id.clone(),
            is_async,
            status,
            model: stats.model.clone().or(meta.model),
            message_count: stats.message_count,
            tool_call_count: stats.tool_call_count,
            tokens: stats.tokens,
            started_ms: stats.started_ms,
            ended_ms: stats.ended_ms,
            prompt_preview: input
                .prompt
                .map(|p| nodes::cap_text(p.trim(), PREVIEW_BYTES, usize::MAX).0),
            final_text,
        });
    }
    links
}

/// Inside an agent file (Subagent scope): tool_use id → nested run, by the run's `toolUseId`,
/// else by `toolUseResult.agentId` in this file.
pub fn nested_links(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    nested: &[SubagentRun],
) -> HashMap<String, AgentId> {
    let mut out = HashMap::new();
    let results = agent_results(s, files);
    for run in nested {
        let tool = run
            .tool_use_id
            .clone()
            .filter(|t| s.tool_use_at.contains_key(t))
            .or_else(|| results.by_agent.get(&run.agent_id).cloned())
            .or_else(|| {
                run.name
                    .as_ref()
                    .and_then(|n| results.by_name.get(n).cloned())
            });
        if let Some(t) = tool {
            out.entry(t).or_insert_with(|| run.agent_id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_launch_kinds() {
        assert_eq!(
            background_launch("Async agent launched successfully.\nagentId: a1b2-c3 (use it…)"),
            Some(("agent", "a1b2-c3".into()))
        );
        assert_eq!(
            background_launch("Command running in background with ID: bx9z. Output…"),
            Some(("shell", "bx9z".into()))
        );
        assert_eq!(
            background_launch("Workflow launched in background. Task ID: w_42\n"),
            Some(("workflow", "w_42".into()))
        );
        assert_eq!(
            background_launch(
                "ok\nCommand did not complete within its 120s timeout and was moved to the background (ID: b1j16ksd4). Output is being written to: /x"
            ),
            Some(("shell", "b1j16ksd4".into()))
        );
        assert_eq!(
            background_launch(
                "Monitor started (task b6wmxusge, timeout 900000ms). You will be notified on each event."
            ),
            Some(("monitor", "b6wmxusge".into()))
        );
        assert_eq!(background_launch("done"), None);
    }
}
