//! Process Files: `<claude config>/sessions/<pid>.json` written by every running Claude Code.
//! The struct and the liveness check (pid plus `procStart`) are the transcript crate's.

pub use ::transcript::live::{ProcessFile, is_alive};
use serde::Deserialize;

use crate::locate::Roots;

/// What a Process File says about background Sessions (the agents page, 2.1.318): a background
/// claude has `kind: "bg"` and its `jobId` (a spare one, kept ready by the daemon, also
/// `spare: true`); the Pane's claude whose conversation moved to the background has `parkedJobId`,
/// the job that conversation runs as (docs/probes.md).
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub parked_job_id: Option<String>,
    #[serde(default)]
    pub spare: Option<bool>,
}

impl Job {
    /// A live background claude running job `job` (not a spare waiting to be claimed).
    pub fn runs(&self, pf: &ProcessFile, job: &str) -> bool {
        self.kind.as_deref() == Some("bg")
            && self.job_id.as_deref() == Some(job)
            && self.spare != Some(true)
            && is_alive(pf)
    }
}

pub fn read(roots: &Roots, pid: u32) -> Option<ProcessFile> {
    read_with_job(roots, pid).map(|(pf, _)| pf)
}

/// The Process File of `pid` with its [`Job`] fields.
pub fn read_with_job(roots: &Roots, pid: u32) -> Option<(ProcessFile, Job)> {
    parse(&std::fs::read(roots.sessions_dir().join(format!("{pid}.json"))).ok()?)
}

fn parse(bytes: &[u8]) -> Option<(ProcessFile, Job)> {
    Some((
        serde_json::from_slice(bytes).ok()?,
        serde_json::from_slice(bytes).ok()?,
    ))
}

/// Every Process File with its [`Job`] fields.
pub fn all_with_job(roots: &Roots) -> Vec<(ProcessFile, Job)> {
    let Ok(rd) = std::fs::read_dir(roots.sessions_dir()) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| parse(&std::fs::read(e.path()).ok()?))
        .collect()
}

/// The Process File of `pid` while it still runs job `job` (see [`Job::runs`]): not a pid reused
/// by another process, nor a Process File left by another claude on the same pid.
pub fn job_process_at(roots: &Roots, pid: u32, job: &str) -> Option<ProcessFile> {
    let (pf, j) = read_with_job(roots, pid)?;
    (j.runs(&pf, job) && pf.pid == Some(pid)).then_some(pf)
}

/// The live background claude running job `job` (see [`Job::runs`]). Its pid changes when the
/// job is stopped and opened again.
pub fn job_process(roots: &Roots, job: &str) -> Option<ProcessFile> {
    all_with_job(roots)
        .into_iter()
        .find_map(|(pf, j)| j.runs(&pf, job).then_some(pf))
}
