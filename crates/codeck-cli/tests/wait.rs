//! `wait` over process files written by hand; the test process is the live pid they name.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const SID: &str = "33333333-4444-4555-8666-777777777777";

fn write_process_file(root: &Path, status: &str, waiting_for: Option<&str>) {
    let pid = std::process::id();
    let file = root.join("sessions").join(format!("{pid}.json"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let body = json!({"pid": pid, "sessionId": SID, "status": status, "waitingFor": waiting_for});
    fs::write(file, body.to_string()).unwrap();
}

/// `wait` with no index at all: `--cache-dir` names a directory that does not exist.
fn wait(root: &Path, timeout: &str) -> Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_codeck"))
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", root)
        .args([
            "--cache-dir",
            "/no/such/cache",
            "wait",
            SID,
            "--timeout",
            timeout,
        ])
        .output()
        .unwrap()
}

fn parsed(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn returns_at_once_when_idle() {
    let root = tempfile::tempdir().unwrap();
    write_process_file(root.path(), "idle", None);
    let out = wait(root.path(), "30");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(parsed(&out)["status"], "idle");
}

#[test]
fn returns_the_reason_when_waiting() {
    let root = tempfile::tempdir().unwrap();
    write_process_file(root.path(), "waiting", Some("permission prompt"));
    let out = wait(root.path(), "30");
    assert_eq!(out.status.code(), Some(0));
    let v = parsed(&out);
    assert_eq!(v["sessionId"], SID);
    assert_eq!(v["status"], "waiting");
    assert_eq!(v["waitingFor"], "permission prompt");
}

#[test]
fn an_absent_process_file_means_ended() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("projects").join("p");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{SID}.jsonl")), "").unwrap();
    let out = wait(root.path(), "30");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(parsed(&out)["status"], "ended");
}

#[test]
fn an_unknown_id_is_an_error_not_ended() {
    let root = tempfile::tempdir().unwrap();
    let out = wait(root.path(), "30");
    assert_eq!(out.status.code(), Some(1));
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["code"], "not_found");
}

#[test]
fn a_timeout_exits_124_with_the_current_status() {
    let root = tempfile::tempdir().unwrap();
    write_process_file(root.path(), "busy", None);
    let out = wait(root.path(), "1");
    assert_eq!(out.status.code(), Some(124));
    assert_eq!(parsed(&out)["status"], "busy");
}

#[test]
fn a_process_file_missing_for_one_read_is_not_ended() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        write_process_file(&path, "waiting", Some("input needed"));
    });
    let out = wait(root.path(), "30");
    writer.join().unwrap();
    assert_eq!(parsed(&out)["status"], "waiting");
}

#[test]
fn notices_a_session_that_stops_working() {
    let root = tempfile::tempdir().unwrap();
    write_process_file(root.path(), "busy", None);
    let started = Instant::now();
    let path = root.path().to_path_buf();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(1200));
        write_process_file(&path, "idle", None);
    });
    let out = wait(root.path(), "30");
    writer.join().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(parsed(&out)["status"], "idle");
    assert!(started.elapsed() < Duration::from_secs(10));
}
