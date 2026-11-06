//! Process Files: `<claude config>/sessions/<pid>.json` written by every running Claude Code.
//! The struct and the liveness check (pid plus `procStart`) are the transcript crate's.

pub use ::transcript::live::{ProcessFile, is_alive};
use serde::Deserialize;

use crate::locate::Roots;

/// What a Process File says about background Sessions (the agents page, 2.1.318): a background
/// claude has `kind: "bg"` and its `jobId`; the Pane's claude whose conversation moved to the
/// background has `parkedJobId`, the job that conversation runs as (docs/probes.md).
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub parked_job_id: Option<String>,
}

pub fn read(roots: &Roots, pid: u32) -> Option<ProcessFile> {
    read_with_job(roots, pid).map(|(pf, _)| pf)
}

/// The Process File of `pid` with its [`Job`] fields.
pub fn read_with_job(roots: &Roots, pid: u32) -> Option<(ProcessFile, Job)> {
    let bytes = std::fs::read(roots.sessions_dir().join(format!("{pid}.json"))).ok()?;
    Some((
        serde_json::from_slice(&bytes).ok()?,
        serde_json::from_slice(&bytes).ok()?,
    ))
}

/// The live background claude running job `job` (`kind: "bg"` with that `jobId`). Its pid changes
/// when the job is stopped and opened again.
pub fn job_process(roots: &Roots, job: &str) -> Option<ProcessFile> {
    let rd = std::fs::read_dir(roots.sessions_dir()).ok()?;
    rd.flatten().find_map(|e| {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            return None;
        }
        let bytes = std::fs::read(&p).ok()?;
        let j: Job = serde_json::from_slice(&bytes).ok()?;
        if j.kind.as_deref() != Some("bg") || j.job_id.as_deref() != Some(job) {
            return None;
        }
        let pf: ProcessFile = serde_json::from_slice(&bytes).ok()?;
        is_alive(&pf).then_some(pf)
    })
}
