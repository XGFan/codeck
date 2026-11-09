//! Subagent meta parsing, linking (toolUseId → agentId → name) and `SubagentRun`.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::nodes::{self, PREVIEW_BYTES, RESULT_BYTES, RESULT_LINES};
use super::{EntryKind, LoadedFile, SessionSidecar, SessionSkeleton, prompt};
use crate::known::{AGENT_TOOLS, RUN_STARTING_TOOLS};
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

/// Facts from the results of the tool calls in one file that can start a Subagent Run.
#[derive(Default)]
struct AgentResults {
    /// `toolUseResult.agentId` → tool_use id.
    by_agent: HashMap<String, String>,
    /// `toolUseResult.name` (teammates) → tool_use id.
    by_name: HashMap<String, String>,
    /// tool_use id → `toolUseResult.status`.
    status: HashMap<String, String>,
    /// tool_use id → `toolUseResult.color` (teammates).
    color: HashMap<String, String>,
    /// tool_use id → `toolUseResult.result` (forked Skills).
    forked_text: HashMap<String, String>,
}

fn agent_results(s: &SessionSkeleton, files: &[LoadedFile]) -> AgentResults {
    let mut out = AgentResults::default();
    for (i, n) in s.nodes.iter().enumerate() {
        let Some(id) = n.tool_results.iter().find(|id| {
            s.tool_use_at.get(*id).is_some_and(|&u| {
                s.nodes[u]
                    .tool_uses
                    .iter()
                    .any(|(tid, name)| tid == *id && RUN_STARTING_TOOLS.contains(&name.as_str()))
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
        let status = match r.status {
            // `forked` says how a Skill ran, not how far it got: in the background it ends with
            // its notification like an async Agent, in the foreground with this result.
            Some(st) if st == "forked" => {
                if let Some(text) = r.result {
                    out.forked_text.insert(id.clone(), text);
                }
                (r.background == Some(true)).then(|| "async_launched".to_owned())
            }
            st => st,
        };
        if let Some(st) = status {
            out.status.insert(id.clone(), st);
        }
        if let Some(c) = r.color {
            out.color.insert(id.clone(), c);
        }
    }
    out
}

/// A teammate as the Main Line tells it: the messages relayed from it and the lead's to it.
#[derive(Debug, Clone, PartialEq)]
struct Teammate {
    /// `running`, `idle` (its round ended; a message from the lead wakes it) or `exited`.
    state: &'static str,
    /// Its latest report, or the result of its latest round.
    last_text: Option<String>,
    color: Option<String>,
}

impl Default for Teammate {
    fn default() -> Self {
        Teammate {
            state: "running",
            last_text: None,
            color: None,
        }
    }
}

/// Teammates by the Agent call that spawned them; a name reused after a shutdown is a new one.
#[derive(Debug, Default)]
struct Teammates {
    by_tool: HashMap<String, Teammate>,
    /// Name → tool_use id of its latest spawn.
    current: HashMap<String, String>,
}

impl Teammates {
    fn spawned(&mut self, name: &str, tool_use_id: &str) {
        self.current.insert(name.to_owned(), tool_use_id.to_owned());
        self.by_tool
            .insert(tool_use_id.to_owned(), Teammate::default());
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Teammate> {
        let tool = self.current.get(name)?;
        self.by_tool.get_mut(tool)
    }

    /// The lead sent `to` (a name, or `*` for all) a message: it works on it.
    fn sent(&mut self, to: &str) {
        let tools: Vec<String> = self
            .current
            .iter()
            .filter(|(n, _)| to == "*" || n.as_str() == to)
            .map(|(_, t)| t.clone())
            .collect();
        for t in tools {
            if let Some(m) = self.by_tool.get_mut(&t).filter(|m| m.state != "exited") {
                m.state = "running";
            }
        }
    }

    /// One `<teammate-message>` relayed to the lead: a report (plain text), the end of a round
    /// (`idle_notification`, with the round's `result` since 2.1.251), or a shutdown
    /// (`shutdown_approved` from it, `teammate_terminated` from `system`: "X has shut down.").
    fn message(&mut self, m: &prompt::TeammateMessage) {
        let json = serde_json::from_str::<serde_json::Value>(&m.body).ok();
        let field = |k: &str| {
            json.as_ref()
                .and_then(|v| v.get(k))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        };
        let from = || field("from").or_else(|| m.teammate_id.clone());
        let (name, state, text) = match json
            .as_ref()
            .map(|v| v.get("type").and_then(|t| t.as_str()))
        {
            None => (m.teammate_id.clone(), "running", Some(m.body.clone())),
            Some(Some("idle_notification")) => (from(), "idle", field("result")),
            Some(Some("shutdown_approved")) => (from(), "exited", None),
            Some(Some("teammate_terminated")) => (
                field("message").and_then(|t| {
                    t.trim_end_matches('.')
                        .strip_suffix(" has shut down")
                        .map(str::to_owned)
                }),
                "exited",
                None,
            ),
            Some(_) => return,
        };
        let system = m.teammate_id.as_deref() == Some("system");
        let Some(t) = name.and_then(|n| self.get_mut(&n)) else {
            return;
        };
        if t.state != "exited" {
            t.state = state;
        }
        if text.is_some() {
            t.last_text = text;
        }
        if t.color.is_none() && !system {
            t.color = m.color.clone();
        }
    }
}

/// The Session's teammates, read along the Main Line in order.
fn teammates(s: &SessionSkeleton, files: &[LoadedFile]) -> Teammates {
    let mut out = Teammates::default();
    let input_field = |id: &str, k: &str| -> Option<String> {
        let (_, b) = s.tool_use_block(files, id)?;
        let v: serde_json::Value = serde_json::from_str(b.input.as_deref()?.get()).ok()?;
        v.get(k)?.as_str().map(str::to_owned)
    };
    for (i, n) in s.nodes.iter().enumerate() {
        match n.entry_type {
            EntryKind::Assistant => {
                for (id, tool) in &n.tool_uses {
                    if AGENT_TOOLS.contains(&tool.as_str()) {
                        if let Some(name) = input_field(id, "name") {
                            out.spawned(&name, id);
                        }
                    } else if tool == "SendMessage"
                        && let Some(to) = input_field(id, "to")
                    {
                        out.sent(&to);
                    }
                }
            }
            EntryKind::User if !n.is_tool_result && !n.is_human => {
                let text = prompt::user_text(s.entry(files, i));
                if text.contains("<teammate-message") {
                    for m in prompt::teammate_messages(&text) {
                        out.message(&m);
                    }
                }
            }
            _ => {}
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
    // A forked Skill's tool_result wraps the text in "Skill … completed (forked execution)."
    let forked = results
        .forked_text
        .get(tool_use_id)
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| nodes::cap_text(t, RESULT_BYTES, RESULT_LINES).0);
    let text = forked
        .or_else(|| result.and_then(|(t, _)| t))
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
    let mates = teammates(s, files);
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
        let (is_async, mut status, mut final_text) = match main_tool.as_deref() {
            Some(t) => outcome(s, files, &results, &notes, t, &m.agent_id),
            None => (false, None, None),
        };
        // A teammate's tool_result only says it was spawned: how it is doing is in the
        // messages it relays to the lead.
        let teammate = main_tool
            .as_deref()
            .filter(|t| results.status.get(*t).map(String::as_str) == Some("teammate_spawned"));
        let mut color = None;
        if let Some(t) = teammate {
            let mate = mates.by_tool.get(t).cloned().unwrap_or_default();
            status = Some(mate.state.to_owned());
            final_text = mate
                .last_text
                .map(|t| nodes::cap_text(t.trim(), RESULT_BYTES, RESULT_LINES).0);
            color = mate.color.or_else(|| results.color.get(t).cloned());
        }
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
            teammate: teammate.is_some(),
            color,
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

    fn msg(id: &str, color: Option<&str>, body: &str) -> prompt::TeammateMessage {
        prompt::TeammateMessage {
            teammate_id: Some(id.into()),
            color: color.map(Into::into),
            body: body.into(),
        }
    }

    #[test]
    fn a_teammate_goes_idle_wakes_and_exits() {
        let mut t = Teammates::default();
        t.spawned("line-a", "toolu_a");
        let now = |t: &Teammates| t.by_tool["toolu_a"].clone();
        assert_eq!(now(&t).state, "running");
        t.message(&msg("line-a", Some("blue"), "Stage one done."));
        assert_eq!(now(&t).last_text.as_deref(), Some("Stage one done."));
        assert_eq!(now(&t).color.as_deref(), Some("blue"));
        // Before 2.1.251 an idle notification has no result: the report stays the latest text.
        t.message(&msg(
            "line-a",
            None,
            r#"{"type":"idle_notification","from":"line-a","idleReason":"available"}"#,
        ));
        assert_eq!(now(&t).state, "idle");
        assert_eq!(now(&t).last_text.as_deref(), Some("Stage one done."));
        t.sent("line-a");
        assert_eq!(now(&t).state, "running");
        t.message(&msg(
            "line-a",
            Some("blue"),
            r#"{"type":"idle_notification","from":"line-a","result":"Sidecars now follow."}"#,
        ));
        assert_eq!(
            (now(&t).state, now(&t).last_text.as_deref()),
            ("idle", Some("Sidecars now follow."))
        );
        t.message(&msg(
            "system",
            None,
            r#"{"type":"teammate_terminated","message":"line-a has shut down."}"#,
        ));
        assert_eq!(now(&t).state, "exited");
        // Exited stays exited; a message to everyone does not wake it.
        t.sent("*");
        t.message(&msg(
            "line-a",
            None,
            r#"{"type":"shutdown_approved","from":"line-a"}"#,
        ));
        assert_eq!(now(&t).state, "exited");
        // A name spawned again is a new teammate.
        t.spawned("line-a", "toolu_b");
        assert_eq!(t.by_tool["toolu_b"].state, "running");
        assert_eq!(now(&t).state, "exited");
        // Unknown names and message types change nothing.
        t.message(&msg("ghost", None, "hi"));
        t.message(&msg(
            "line-a",
            None,
            r#"{"type":"task_assignment","taskId":"6"}"#,
        ));
        assert_eq!(t.by_tool["toolu_b"], Teammate::default());
    }
}
