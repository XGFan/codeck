//! `wait`: blocks until a Session stops working. Only the process files: no index, so a Session
//! just started is seen at once.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use codeck_core::model::LiveStatus;
use codeck_core::paths;
use serde::Serialize;
use transcript::live;

use crate::args::Parsed;
use crate::common::{self, CliError, Context};

/// Under the 120 s Bash tool default: the caller gets an answer and can call again.
const DEFAULT_TIMEOUT_SECS: u64 = 110;
/// Adding an absurd timeout to an `Instant` would panic.
const MAX_TIMEOUT_SECS: u64 = 86_400;
const POLL: Duration = Duration::from_secs(1);
const TIMED_OUT: i32 = 124;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    session_id: String,
    status: &'static str,
    waiting_for: Option<String>,
}

pub fn run(cx: &Context, a: &Parsed) -> Result<(Output, i32), CliError> {
    let id = a.one_positional("Session ID")?;
    let secs = a
        .number("--timeout", Some(DEFAULT_TIMEOUT_SECS))?
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);
    let root = &cx.data_root().path;
    let result = wait(root, id, Duration::from_secs(secs));
    // No process file also describes an id that never existed; a caller must not read that as finished.
    if result.0.status == "ended" && !has_transcript(root, id) {
        return Err(CliError::new("not_found", "没有这个 Session"));
    }
    Ok(result)
}

fn has_transcript(root: &Path, id: &str) -> bool {
    let name = format!("{id}.jsonl");
    std::fs::read_dir(paths::projects_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .any(|project| project.path().join(&name).is_file())
}

fn wait(root: &Path, id: &str, timeout: Duration) -> (Output, i32) {
    let deadline = Instant::now() + timeout;
    let sessions = paths::sessions_dir(root);
    // A process file being rewritten can read as absent for a moment: ended only if it stays so.
    let mut absent = 0;
    loop {
        let live = live::detect(&live::read_process_files(&sessions));
        let state = live.get(id);
        let busy = state.is_some_and(|s| s.status == LiveStatus::Busy);
        absent = if state.is_none() { absent + 1 } else { 0 };
        let settled = !busy && absent != 1;
        if settled || Instant::now() >= deadline {
            let code = if busy { TIMED_OUT } else { 0 };
            return (
                Output {
                    session_id: id.to_owned(),
                    status: common::status_word(state),
                    waiting_for: state.and_then(|s| s.waiting_for.clone()),
                },
                code,
            );
        }
        thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}
