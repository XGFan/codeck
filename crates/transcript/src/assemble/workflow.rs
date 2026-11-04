//! `WorkflowRun` from `workflows/<runId>.json`, falling back to `journal.jsonl` (F12).

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::nodes::{PREVIEW_BYTES, cap_text};
use super::{LoadedFile, SessionSidecar, SessionSkeleton, WorkflowFiles};
use crate::model::{WorkflowAgent, WorkflowPhase, WorkflowRun};
use crate::raw::lenient;

/// Builds every Workflow Run of the sidecar and maps main-file `Workflow` tool_use ids to run ids
/// (via `toolUseResult.runId`).
pub fn runs(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    sidecar: &SessionSidecar,
) -> (Vec<WorkflowRun>, HashMap<String, String>) {
    let mut by_tool = HashMap::new();
    let mut tool_by_run: HashMap<String, String> = HashMap::new();
    for (i, n) in s.nodes.iter().enumerate() {
        let Some(id) = n.tool_results.iter().find(|id| {
            s.tool_use_at.get(*id).is_some_and(|&u| {
                s.nodes[u]
                    .tool_uses
                    .iter()
                    .any(|(tid, name)| tid == *id && name == "Workflow")
            })
        }) else {
            continue;
        };
        if let Some(run_id) = s
            .entry(files, i)
            .tool_use_result_fields()
            .and_then(|r| r.run_id)
        {
            by_tool.entry(id.clone()).or_insert_with(|| run_id.clone());
            tool_by_run.entry(run_id).or_insert_with(|| id.clone());
        }
    }
    let runs = sidecar
        .workflows
        .iter()
        .map(|w| {
            let mut run = build_run(w, sidecar);
            run.tool_use_id = tool_by_run.get(&w.run_id).cloned();
            run
        })
        .collect();
    (runs, by_tool)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WorkflowJson<'a> {
    #[serde(default, deserialize_with = "lenient::string")]
    workflow_name: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    summary: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    status: Option<String>,
    #[serde(default, borrow)]
    phases: Option<&'a RawValue>,
    #[serde(default, borrow)]
    workflow_progress: Option<&'a RawValue>,
    #[serde(default, deserialize_with = "lenient::number")]
    total_tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    duration_ms: Option<f64>,
}

#[derive(Deserialize, Default)]
struct PhaseJson {
    #[serde(default, deserialize_with = "lenient::string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    detail: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ProgressJson {
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    index: Option<f64>,
    #[serde(default, deserialize_with = "lenient::string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    label: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    phase_index: Option<f64>,
    #[serde(default, deserialize_with = "lenient::string")]
    state: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    tool_calls: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    duration_ms: Option<f64>,
    #[serde(default, deserialize_with = "lenient::string")]
    result_preview: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct JournalLine<'a> {
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    agent_id: Option<String>,
    #[serde(default, borrow)]
    result: Option<&'a RawValue>,
}

fn list<T: DeserializeOwned + Default>(raw: Option<&RawValue>) -> Vec<T> {
    raw.and_then(|r| serde_json::from_str::<Vec<serde_json::Value>>(r.get()).ok())
        .map(|items| {
            items
                .into_iter()
                .map(|v| T::deserialize(v).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

fn count(v: Option<f64>) -> u32 {
    v.map(|x| x.max(0.0) as u32).unwrap_or(0)
}

/// One Workflow Run from its own files alone, for readers following a live Session (no index, so
/// no per-agent figures beyond what the files carry; `tool_use_id` is left unset).
pub fn run_from_files(w: &WorkflowFiles) -> WorkflowRun {
    build_run(w, &SessionSidecar::default())
}

fn build_run(w: &WorkflowFiles, sidecar: &SessionSidecar) -> WorkflowRun {
    let mut run = WorkflowRun {
        run_id: w.run_id.clone(),
        name: None,
        summary: None,
        status: None,
        tool_use_id: None,
        phases: Vec::new(),
        agents: Vec::new(),
        duration_ms: None,
        total_tokens: None,
    };
    if let Some(json) = w
        .workflow_json
        .as_deref()
        .and_then(|j| serde_json::from_str::<WorkflowJson<'_>>(j).ok())
    {
        run.name = json.workflow_name;
        run.summary = json.summary;
        run.status = json.status;
        run.duration_ms = json.duration_ms;
        run.total_tokens = json.total_tokens;
        let progress: Vec<ProgressJson> = list(json.workflow_progress);
        let phases: Vec<PhaseJson> = list(json.phases);
        run.phases = phases
            .into_iter()
            .enumerate()
            .map(|(k, p)| WorkflowPhase {
                index: k as u32 + 1,
                title: p.title.unwrap_or_default(),
                detail: p.detail,
            })
            .collect();
        if run.phases.is_empty() {
            run.phases = progress
                .iter()
                .filter(|p| p.kind.as_deref() == Some("workflow_phase"))
                .map(|p| WorkflowPhase {
                    index: count(p.index),
                    title: p.title.clone().unwrap_or_default(),
                    detail: None,
                })
                .collect();
        }
        run.agents = progress
            .into_iter()
            .filter(|p| p.kind.as_deref() == Some("workflow_agent"))
            .filter_map(|p| {
                Some(WorkflowAgent {
                    agent_id: p.agent_id?,
                    label: p.label,
                    phase_index: p.phase_index.map(|x| x.max(0.0) as u32),
                    state: p.state,
                    model: p.model,
                    tokens: p.tokens.unwrap_or(0.0),
                    tool_calls: count(p.tool_calls),
                    duration_ms: p.duration_ms.unwrap_or(0.0),
                    result_preview: p.result_preview,
                })
            })
            .collect();
        return run;
    }
    // Journal fallback (run still in progress): one agent per `started`, done once a `result` exists.
    let Some(journal) = w.journal_jsonl.as_deref() else {
        return run;
    };
    let mut order: Vec<String> = Vec::new();
    let mut results: HashMap<String, Option<String>> = HashMap::new();
    for line in journal.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(j) = serde_json::from_str::<JournalLine<'_>>(line) else {
            continue;
        };
        let Some(agent) = j.agent_id else {
            continue;
        };
        if !order.contains(&agent) {
            order.push(agent.clone());
        }
        if j.kind.as_deref() == Some("result") || j.result.is_some() {
            let preview = j
                .result
                .map(|r| cap_text(r.get(), PREVIEW_BYTES, usize::MAX).0);
            results.insert(agent, preview);
        }
    }
    let running = order.iter().any(|a| !results.contains_key(a));
    run.status = Some(if running { "running" } else { "completed" }.to_owned());
    run.agents = order
        .into_iter()
        .map(|agent_id| {
            let stats = sidecar
                .agent_stats
                .get(&agent_id)
                .cloned()
                .unwrap_or_default();
            let done = results.get(&agent_id);
            WorkflowAgent {
                label: None,
                phase_index: None,
                state: Some(if done.is_some() { "done" } else { "running" }.to_owned()),
                model: stats.model,
                tokens: stats.tokens.input + stats.tokens.output,
                tool_calls: stats.tool_call_count,
                duration_ms: match (stats.started_ms, stats.ended_ms) {
                    (Some(a), Some(b)) => b - a,
                    _ => 0.0,
                },
                result_preview: done.cloned().flatten(),
                agent_id,
            }
        })
        .collect();
    run
}
