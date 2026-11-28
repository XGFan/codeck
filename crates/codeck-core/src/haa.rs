//! Cached read-only haa Task records from `<haa_home>/usage.jsonl`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::model::HaaTask;

type FileStamp = Option<(SystemTime, u64)>;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UsageRow {
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    worker: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    duration_ms: Option<f64>,
    #[serde(default)]
    session_id: Option<String>,
    /// The backend's version, recorded under the backend's name: which backend ran the Task.
    #[serde(default)]
    agy: Option<serde_json::Value>,
    #[serde(default)]
    codex: Option<serde_json::Value>,
}

/// Which backend ran a Task: the model's `<backend>/` prefix, else the version field haa records
/// under the backend's name. A record with neither is from before haa recorded versions
/// (2026-11-04), when pi was its only backend.
fn task_backend(row: &UsageRow) -> &'static str {
    match row
        .model
        .as_deref()
        .and_then(|m| m.split_once('/'))
        .map(|(p, _)| p)
    {
        Some("agy") => "agy",
        Some("codex") => "codex",
        Some("pi") => "pi",
        _ if row.agy.is_some() => "agy",
        _ if row.codex.is_some() => "codex",
        _ => "pi",
    }
}

/// Task records grouped by owner, cached by path and file (mtime, size).
#[derive(Debug, Default)]
pub struct HaaCache {
    haa_home: Option<PathBuf>,
    usage_stamp: FileStamp,
    tasks: HashMap<String, Vec<HaaTask>>,
}

impl HaaCache {
    fn refresh(&mut self, haa_home: &Path) {
        let usage_path = haa_home.join("usage.jsonl");
        let usage_stamp = file_stamp(&usage_path);

        if self.haa_home.as_deref() == Some(haa_home) && self.usage_stamp == usage_stamp {
            return;
        }

        self.tasks = read_usage(&usage_path);
        self.haa_home = Some(haa_home.to_path_buf());
        self.usage_stamp = usage_stamp;
    }

    /// Returns cached haa Worker Tasks owned by `owner`, ordered by `ended_ms` ascending.
    pub fn tasks_for(&mut self, haa_home: &Path, owner: &str) -> &[HaaTask] {
        self.refresh(haa_home);
        self.tasks.get(owner).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

fn parse_ts_ms(ts: Option<&str>) -> Option<f64> {
    let raw = ts.filter(|s| !s.is_empty())?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis() as f64)
}

fn read_usage(path: &Path) -> HashMap<String, Vec<HaaTask>> {
    let mut tasks: HashMap<String, Vec<HaaTask>> = HashMap::new();
    let Ok(file) = File::open(path) else {
        return tasks;
    };
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<UsageRow>(&line) else {
            continue;
        };
        if row.kind.as_deref() != Some("task") {
            continue;
        }
        let backend = task_backend(&row).to_owned();
        let owner = non_empty(row.owner);
        let worker = non_empty(row.worker);
        let title = non_empty(row.title);
        let session_id = non_empty(row.session_id);

        if let (Some(owner_id), Some(ended_ms)) = (owner, parse_ts_ms(row.ts.as_deref())) {
            tasks.entry(owner_id).or_default().push(HaaTask {
                worker,
                session_id,
                title,
                task: non_empty(row.task),
                status: non_empty(row.status),
                model: non_empty(row.model),
                backend,
                ended_ms,
                duration_ms: row.duration_ms.filter(|v| v.is_finite()),
            });
        }
    }
    for list in tasks.values_mut() {
        list.sort_by(|a, b| a.ended_ms.total_cmp(&b.ended_ms));
    }
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn tasks_group_by_owner_sort_by_ended_ms_and_skip_invalid_rows() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("usage.jsonl"),
            [
                "not json",
                // Non-task kind ignored.
                r#"{"ts":"2026-11-27T01:00:00Z","kind":"spawn","owner":"m1","worker":"w0"}"#,
                // Missing / invalid ts skipped for tasks.
                r#"{"kind":"task","owner":"m1","worker":"no-ts"}"#,
                r#"{"ts":"not-a-timestamp","kind":"task","owner":"m1","worker":"bad-ts"}"#,
                // Missing / empty owner skipped.
                r#"{"ts":"2026-11-27T01:00:00Z","kind":"task","worker":"no-owner"}"#,
                r#"{"ts":"2026-11-27T01:00:00Z","kind":"task","owner":"","worker":"empty-owner"}"#,
                // Later task written earlier in the file -> sorted after earlier task.
                r#"{"ts":"2026-11-27T01:05:00.500Z","kind":"task","owner":"m1","worker":"w2","title":"second","task":"do 2","status":"done","model":"google/gemini-4-neon-high","durationMs":120000,"sessionId":"s2","agy":"1.5.18"}"#,
                // Earlier task without sessionId, with empty strings normalized to None.
                r#"{"ts":"2026-11-27T01:02:00Z","kind":"task","owner":"m1","worker":"w1","title":"","task":"do 1","status":"failed","model":"","sessionId":""}"#,
                // Different owner.
                r#"{"ts":"2026-11-27T01:03:00Z","kind":"task","owner":"m2","worker":"w3","title":"other","status":"aborted","durationMs":5000}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut cache = HaaCache::default();
        let m1 = cache.tasks_for(dir.path(), "m1");
        assert_eq!(m1.len(), 2);
        assert_eq!(
            m1[0],
            HaaTask {
                worker: Some("w1".into()),
                session_id: None,
                title: None,
                task: Some("do 1".into()),
                status: Some("failed".into()),
                model: None,
                backend: "pi".into(),
                ended_ms: chrono::DateTime::parse_from_rfc3339("2026-11-27T01:02:00Z")
                    .unwrap()
                    .timestamp_millis() as f64,
                duration_ms: None,
            }
        );
        assert_eq!(
            m1[1],
            HaaTask {
                worker: Some("w2".into()),
                session_id: Some("s2".into()),
                title: Some("second".into()),
                task: Some("do 2".into()),
                status: Some("done".into()),
                model: Some("google/gemini-4-neon-high".into()),
                backend: "agy".into(),
                ended_ms: chrono::DateTime::parse_from_rfc3339("2026-11-27T01:05:00.500Z")
                    .unwrap()
                    .timestamp_millis() as f64,
                duration_ms: Some(120_000.0),
            }
        );

        let m2 = cache.tasks_for(dir.path(), "m2");
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0].worker.as_deref(), Some("w3"));
        assert_eq!(m2[0].status.as_deref(), Some("aborted"));
        assert_eq!(m2[0].duration_ms, Some(5_000.0));

        assert!(cache.tasks_for(dir.path(), "unknown").is_empty());
    }

    #[test]
    fn task_backend_from_model_prefix_then_version_field() {
        let row = |s: &str| serde_json::from_str::<UsageRow>(s).unwrap();
        assert_eq!(
            task_backend(&row(r#"{"model":"codex/gpt-6.2-sol:high"}"#)),
            "codex"
        );
        assert_eq!(
            task_backend(&row(r#"{"model":"pi/agent-gateway/deepseek-v4.1-flash"}"#)),
            "pi"
        );
        assert_eq!(
            task_backend(&row(r#"{"model":"agy/gemini-4-neon-high","codex":"x"}"#)),
            "agy"
        );
        // Before the Agent / Model split the model had no backend; the version field tells.
        assert_eq!(
            task_backend(&row(r#"{"model":"gpt-6.2-sol","codex":"0.171.0"}"#)),
            "codex"
        );
        assert_eq!(
            task_backend(&row(r#"{"model":"gemini-4-neon-high","agy":"1.5.18"}"#)),
            "agy"
        );
        assert_eq!(
            task_backend(&row(
                r#"{"model":"agent-gateway/deepseek-v4.1-flash","pi":"1.5.0"}"#
            )),
            "pi"
        );
        // Before haa recorded versions, pi was its only backend.
        assert_eq!(
            task_backend(&row(r#"{"model":"agent-gateway/deepseek-v4.1-flash"}"#)),
            "pi"
        );
    }
}
