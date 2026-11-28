use codeck_core::model::{SessionQuery, SessionSort};
use serde::Serialize;

use crate::args::Parsed;
use crate::common::{self, CliError, Context};

const DEFAULT_LIMIT: u64 = 20;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    indexed_at: Option<String>,
    /// Matching Sessions, before `--limit`.
    total: usize,
    sessions: Vec<Item>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    session_id: String,
    title: String,
    project: Option<String>,
    status: &'static str,
    waiting_for: Option<String>,
    created_at: Option<String>,
    last_active_at: Option<String>,
    message_count: u32,
    git_branch: Option<String>,
}

pub fn run(cx: &Context, a: &Parsed) -> Result<Output, CliError> {
    if !a.positional.is_empty() {
        return Err(CliError::usage("sessions 不收位置参数"));
    }
    let limit = a.number("--limit", Some(DEFAULT_LIMIT))?.unwrap_or(0) as usize;
    let engine = cx.engine()?;
    let project_ids = match a.value("--project") {
        Some(p) => vec![common::find_project(&engine, p)?.id],
        None => Vec::new(),
    };
    let projects = common::project_paths(&engine)?;
    let mut rows = engine.list_sessions(&SessionQuery {
        project_ids,
        sort: SessionSort::LastActive,
        descending: true,
        time_range: None,
        drill: None,
    })?;
    if a.flag("--live") {
        rows.retain(|s| s.live.is_some());
    }
    let total = rows.len();
    let sessions = rows
        .into_iter()
        .take(limit)
        .map(|s| Item {
            status: common::status_word(s.live.as_ref()),
            waiting_for: s.live.and_then(|l| l.waiting_for),
            project: projects.get(&s.project_id).cloned(),
            created_at: common::time_of_ms(s.created_ms),
            last_active_at: common::time_of_ms(s.last_active_ms),
            message_count: s.message_count,
            git_branch: s.git_branch,
            title: s.title,
            session_id: s.id,
        })
        .collect();
    Ok(Output {
        indexed_at: engine.indexed_at().map(common::time_of),
        total,
        sessions,
    })
}
