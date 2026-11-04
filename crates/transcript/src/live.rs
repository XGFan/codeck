//! Live Sessions: `<root>/sessions/*.json` + `kill(pid, 0)` + procStart check.
//!
//! A process file is live when its pid exists and the process started within ±2 s of `procStart`
//! (`ps -o lstart` format; a mismatch means the pid was reused). `status: "busy"` is
//! Busy, `"waiting"` Waiting (with `waitingFor`), any other value Idle with the raw value kept. A Session named by no live process file has
//! ended, however recently its transcript was written: Claude Code removes the file when it exits, so a
//! write time cannot tell a running Session from one that just ended.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Local, NaiveDateTime, TimeZone};
use serde::Deserialize;

use crate::model::{LiveState, LiveStatus};

/// Allowed difference between `procStart` and the real process start.
const START_TOLERANCE_SECS: i64 = 2;

/// One `<root>/sessions/<pid>.json`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessFile {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub proc_start: Option<String>,
    #[serde(default)]
    pub waiting_for: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Reads every `*.json` in `sessions_dir` (other files, e.g. `*.key`, are ignored).
pub fn read_process_files(sessions_dir: &Path) -> Vec<ProcessFile> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        if let Ok(pf) = serde_json::from_slice::<ProcessFile>(&bytes) {
            out.push(pf);
        }
    }
    out
}

/// Parses `procStart` (`Wed Oct  7 09:34:56 2026`) to candidate unix seconds. Real Claude Code
/// 2.1.x files carry UTC while `ps -o lstart` prints local time, so both readings are returned
/// (local ones first; a DST fold yields two).
pub fn parse_proc_start(s: &str) -> Vec<i64> {
    let norm = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let Ok(naive) = NaiveDateTime::parse_from_str(&norm, "%a %b %d %H:%M:%S %Y") else {
        return Vec::new();
    };
    let r = Local.from_local_datetime(&naive);
    let mut v: Vec<i64> = [r.earliest(), r.latest()]
        .into_iter()
        .flatten()
        .map(|d| d.timestamp())
        .collect();
    v.push(naive.and_utc().timestamp());
    v.dedup();
    v
}

/// `kill(pid, 0)`: the process exists (EPERM also means it exists).
pub fn pid_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence/permission check.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Process start time in unix seconds via `proc_pidinfo(PROC_PIDTBSDINFO)`.
#[cfg(target_os = "macos")]
pub fn process_start_secs(pid: u32) -> Option<i64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_bsdinfo is plain old data; the kernel fills at most `size` bytes.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (n == size).then_some(info.pbi_start_tvsec as i64)
}

#[cfg(not(target_os = "macos"))]
pub fn process_start_secs(_pid: u32) -> Option<i64> {
    None
}

/// The process file describes a running process: the pid exists and, when both are known, its
/// start time matches `procStart` within ±2 s.
pub fn is_alive(pf: &ProcessFile) -> bool {
    let Some(pid) = pf.pid else {
        return false;
    };
    if !pid_exists(pid) {
        return false;
    }
    let Some(recorded) = pf.proc_start.as_deref() else {
        return true;
    };
    let candidates = parse_proc_start(recorded);
    if candidates.is_empty() {
        // Unparsable procStart: kill(pid, 0) is all we can check.
        return true;
    }
    match process_start_secs(pid) {
        Some(actual) => candidates
            .iter()
            .any(|c| (c - actual).abs() <= START_TOLERANCE_SECS),
        // On macOS proc_pidinfo only fails (EPERM) for another user's process, which is not this
        // user's Claude Code, i.e. the pid was reused. Elsewhere there is no start-time source.
        None => !cfg!(target_os = "macos"),
    }
}

/// Live state from process files.
pub fn detect(process_files: &[ProcessFile]) -> BTreeMap<String, LiveState> {
    let mut out: BTreeMap<String, LiveState> = BTreeMap::new();
    for pf in process_files {
        let Some(sid) = pf.session_id.as_deref() else {
            continue;
        };
        if !is_alive(pf) {
            continue;
        }
        let status = match pf.status.as_deref() {
            Some("busy") => LiveStatus::Busy,
            Some("waiting") => LiveStatus::Waiting,
            _ => LiveStatus::Idle,
        };
        let state = LiveState {
            status,
            raw_status: pf.status.clone(),
            waiting_for: (status == LiveStatus::Waiting)
                .then(|| pf.waiting_for.clone())
                .flatten(),
            pid: pf.pid,
        };
        // Several live processes on one session: Busy wins.
        match out.get(sid) {
            Some(prev) if prev.status == LiveStatus::Busy => {}
            _ => {
                out.insert(sid.to_owned(), state);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own_process_file(json: &str) -> ProcessFile {
        let mut pf: ProcessFile = serde_json::from_str(json).unwrap();
        pf.pid = Some(std::process::id());
        pf
    }

    #[test]
    fn maps_waiting_and_busy_status() {
        let waiting = own_process_file(
            r#"{"sessionId":"s1","status":"waiting","waitingFor":"permission prompt","cwd":"/x","version":"2.1.0"}"#,
        );
        assert_eq!(waiting.cwd.as_deref(), Some("/x"));
        assert_eq!(waiting.version.as_deref(), Some("2.1.0"));
        let busy = own_process_file(
            r#"{"sessionId":"s2","status":"busy","waitingFor":"permission prompt"}"#,
        );
        let out = detect(&[waiting, busy]);
        assert_eq!(out["s1"].status, LiveStatus::Waiting);
        assert_eq!(out["s1"].raw_status.as_deref(), Some("waiting"));
        assert_eq!(out["s1"].waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(out["s2"].status, LiveStatus::Busy);
        assert_eq!(out["s2"].waiting_for, None);
    }

    #[test]
    fn parses_ps_lstart_format() {
        let v = parse_proc_start("Wed Oct  7 09:34:56 2026");
        assert!(!v.is_empty());
        let d = Local.timestamp_opt(v[0], 0).unwrap();
        assert_eq!(
            d.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-10-07 09:34:56"
        );
        let utc = chrono::NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(9, 34, 56)
            .unwrap()
            .and_utc()
            .timestamp();
        assert!(v.contains(&utc));
        assert!(parse_proc_start("garbage").is_empty());
    }

    #[test]
    fn own_process_start_matches_now_roughly() {
        let start = process_start_secs(std::process::id());
        if cfg!(target_os = "macos") {
            let start = start.expect("own start time");
            let now = chrono::Utc::now().timestamp();
            assert!(start <= now && now - start < 24 * 3600);
        }
        assert!(pid_exists(std::process::id()));
    }
}
