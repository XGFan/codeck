//! Subagent meta parsing, linking (toolUseId → agentId → name) and `SubagentRun` (§3.4 step 11).

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::nodes::{self, PREVIEW_BYTES, RESULT_BYTES, RESULT_LINES};
use super::{LoadedFile, SessionSidecar, SessionSkeleton, prompt};
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

/// A parsed task-notification prompt.
#[derive(Debug, Clone)]
pub struct Notification {
    pub node_id: NodeId,
    pub task_id: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
}

/// Task-notification prompts keyed by their `<tool-use-id>` (F11).
pub fn notification_facts(
    s: &SessionSkeleton,
    files: &[LoadedFile],
) -> HashMap<String, Notification> {
    let mut out = HashMap::new();
    for (i, n) in s.nodes.iter().enumerate() {
        if n.entry_type != "user" || n.is_tool_result || n.is_compact_summary || n.is_human {
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
        if let PromptOrigin::TaskNotification {
            task_id,
            tool_use_id: Some(tool_use_id),
            status,
            summary,
        } = prompt::classify(e, &text)
        {
            out.entry(tool_use_id).or_insert(Notification {
                node_id: n.uuid.clone(),
                task_id,
                status,
                summary,
            });
        }
    }
    out
}

/// tool_use id → task-notification prompt uuid.
pub fn notifications(s: &SessionSkeleton, files: &[LoadedFile]) -> HashMap<String, NodeId> {
    notification_facts(s, files)
        .into_iter()
        .map(|(k, v)| (k, v.node_id))
        .collect()
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
    let Some(&i) = s.tool_use_at.get(tool_use_id) else {
        return AgentInput::default();
    };
    s.entry(files, i)
        .blocks()
        .iter()
        .find(|b| b.id.as_deref() == Some(tool_use_id))
        .and_then(|b| b.input.as_deref())
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .unwrap_or_default()
}

/// Capped text of the tool_result for `tool_use_id`, if any.
fn result_text(s: &SessionSkeleton, files: &[LoadedFile], tool_use_id: &str) -> Option<String> {
    let &r = s.results_for.get(tool_use_id)?.first()?;
    let block = s
        .entry(files, r)
        .blocks()
        .iter()
        .find(|b| b.tool_use_id.as_deref() == Some(tool_use_id))?;
    let (text, _) = nodes::result_content(block.content.as_deref());
    let text = text.trim();
    (!text.is_empty()).then(|| nodes::cap_text(text, RESULT_BYTES, RESULT_LINES).0)
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
        let note = main_tool.as_ref().and_then(|t| notes.get(t));
        let result_status = main_tool
            .as_ref()
            .and_then(|t| results.status.get(t))
            .cloned();
        let is_async = result_status.as_deref() == Some("async_launched");
        let input = main_tool
            .as_deref()
            .map(|t| agent_input(s, files, t))
            .unwrap_or_default();
        let result = main_tool.as_deref().and_then(|t| result_text(s, files, t));
        let summary = note.and_then(|n| n.summary.clone());
        let final_text = if is_async {
            summary.or(result)
        } else {
            result.or(summary)
        };
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
            status: note.and_then(|n| n.status.clone()).or(result_status),
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
