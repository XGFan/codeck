//! Shared test helpers (T1.2). Use from an integration test with `mod common;`.
//!
//! Fixtures live in `tests/fixtures/<scenario>/` as mini data roots (`projects/…`, `sessions/…`);
//! see `tests/fixtures/README.md`. `fixture_root` copies one into a temp dir so tests may mutate it.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

pub const SCENARIOS: &[&str] = &[
    "basic",
    "branch",
    "compact",
    "fork",
    "copies",
    "subagents",
    "workflow",
    "persisted_and_images",
    "live",
];

/// Session ids and encoded project directory names of the fixtures.
pub mod ids {
    pub const BASIC: &str = "86d20d64-9fdd-5e64-aae4-93895b13068d";
    pub const BASIC_DIR: &str = "-Users-dev-code-lumen-api";
    pub const BRANCH: &str = "6d4df888-616c-528f-b5b9-674beb27f393";
    pub const BRANCH_DIR: &str = "-Users-dev-code-orbit-web";
    /// compact: boundary whose logicalParentUuid exists and has the `/compact` prompt as a sibling
    pub const COMPACT_SIBLING: &str = "699c395a-2b04-59eb-bb3a-eb0e0847717b";
    /// compact: boundary whose logicalParentUuid is missing from the file
    pub const COMPACT_MISSING: &str = "d131117c-dd2d-5403-9be8-9b5c492229a9";
    /// compact: boundary whose logicalParentUuid points at a LATER entry (parent cycle)
    pub const COMPACT_CYCLE: &str = "6a5f53ed-a977-58c4-a74d-a5e9d4877d22";
    pub const COMPACT_DIR: &str = "-Users-dev-code-tidepool";
    pub const FORK_ORIGIN: &str = "d3b58148-dbd3-54d8-a4e3-f272df8eac0d";
    pub const FORK_CHILD: &str = "d6ac0c3a-1415-5f66-bbd9-6cc17c0eb4f5";
    pub const FORK_DIR: &str = "-Users-dev-code-tidepool";
    /// copies: byte-identical in both project dirs
    pub const COPIES_IDENTICAL: &str = "2000072b-1616-53ff-b8dd-0b509f44339e";
    /// copies: `-old` holds a byte prefix (2 turns), the main dir the superset (4 turns)
    pub const COPIES_SUPERSET: &str = "d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa";
    pub const COPIES_DIR: &str = "-Users-dev-code-kestrel-cli";
    pub const COPIES_DIR_OLD: &str = "-Users-dev-code-kestrel-cli-old";
    /// name of the symlinked project dir `fixture_root("copies")` adds (points at `COPIES_DIR`)
    pub const COPIES_LINK: &str = "-Users-dev-code-kestrel-cli-link";
    pub const SUBAGENTS: &str = "ee98ea57-9b48-56b0-9860-5c8105c7f967";
    pub const SUBAGENTS_DIR: &str = "-Users-dev-code-tidepool";
    pub const WORKFLOW: &str = "418be169-3e5e-5397-b9f9-862014ebf2fa";
    pub const WORKFLOW_DIR: &str = "-Users-dev-code-kestrel-cli";
    pub const PERSISTED: &str = "3e7603aa-503a-511b-8228-4e9155f1d5e4";
    pub const PERSISTED_DIR: &str = "-Users-dev-code-lumen-api";
    /// live: transcripts exist for these two; the other two sessions json files have none
    pub const LIVE_BUSY: &str = "b5805323-fcce-58f1-bcf9-40e720af88f4";
    pub const LIVE_SHELL: &str = "45470335-35d2-5185-ad2c-5f6ca8463a06";
    pub const LIVE_DEAD: &str = "6c8546d4-6e10-5c8d-9cc3-b6e13f66eef1";
    pub const LIVE_REUSED: &str = "1fc71c8b-e8ea-52c0-83bd-ad683944a26c";
    pub const LIVE_DIR: &str = "-Users-dev-code-orbit-web";
}

/// Path of a scenario inside the repo (read-only; never write here).
pub fn fixture_dir(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}

/// Pids rendered into the `live` sessions dir.
#[derive(Debug, Clone, Copy)]
pub struct LivePids {
    /// this test process: alive, `status:"busy"`, `procStart` matches
    pub pid: u32,
    /// parent process: alive, `status:"shell"`, `procStart` matches
    pub ppid: u32,
    /// a process that has exited: `status:"idle"`
    pub dead_pid: u32,
    /// pid 1 with a wrong `procStart` (pid reuse): not the recorded process
    pub reused_pid: u32,
}

/// Copy scenario `name` into a fresh temp dir and return it; the dir is a data root
/// (`<root>/projects`, `<root>/sessions`).
/// - `copies`: also adds a symlinked project dir `ids::COPIES_LINK -> ids::COPIES_DIR`.
/// - `live`: renders every `sessions/*.template` for the running test process (see `render_live`).
pub fn fixture_root(name: &str) -> TempDir {
    fixture_root_with_pids(name).0
}

/// Like `fixture_root`, also returning the pids rendered for `live` (`None` for other scenarios).
pub fn fixture_root_with_pids(name: &str) -> (TempDir, Option<LivePids>) {
    let src = fixture_dir(name);
    assert!(src.is_dir(), "unknown fixture scenario {name}");
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_dir_all(&src, tmp.path()).expect("copy fixture");
    let mut pids = None;
    match name {
        "copies" => add_symlinked_project(tmp.path(), ids::COPIES_LINK, ids::COPIES_DIR),
        "live" => pids = Some(render_live(tmp.path())),
        _ => {}
    }
    (tmp, pids)
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// `<root>/projects/<link> -> <target>` (relative link, like the real `-Joy-tossh -> -Joy-toh`).
pub fn add_symlinked_project(root: &Path, link: &str, target: &str) {
    let projects = root.join("projects");
    assert!(
        projects.join(target).is_dir(),
        "missing project dir {target}"
    );
    std::os::unix::fs::symlink(target, projects.join(link)).expect("symlink project dir");
}

/// Start time of `pid` in the format Claude Code writes to `procStart`,
/// e.g. `Wed Oct  7 09:34:56 2026` (local time, space-padded day), via `ps -o lstart=`.
pub fn proc_start(pid: u32) -> String {
    let out = Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!s.is_empty(), "ps found no process {pid}");
    s
}

pub fn parent_pid() -> u32 {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("ppid")
}

/// pid of a child that has already exited and been reaped (no such process now).
pub fn dead_pid() -> u32 {
    for _ in 0..5 {
        let mut child = Command::new("true").spawn().expect("spawn true");
        let pid = child.id();
        child.wait().expect("wait true");
        let alive = Command::new("ps")
            .args(["-p", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !alive {
            return pid;
        }
    }
    panic!("could not obtain a dead pid");
}

/// Render `<root>/sessions/*.template`: placeholders `{{PID}}`, `{{PROC_START}}`, `{{PPID}}`,
/// `{{PPID_START}}`, `{{DEAD_PID}}` in both file names and contents; the `.template` suffix is dropped.
pub fn render_live(root: &Path) -> LivePids {
    let pids = LivePids {
        pid: std::process::id(),
        ppid: parent_pid(),
        dead_pid: dead_pid(),
        reused_pid: 1,
    };
    let subs = [
        ("{{PID}}", pids.pid.to_string()),
        ("{{PROC_START}}", proc_start(pids.pid)),
        ("{{PPID}}", pids.ppid.to_string()),
        ("{{PPID_START}}", proc_start(pids.ppid)),
        ("{{DEAD_PID}}", pids.dead_pid.to_string()),
    ];
    let dir = root.join("sessions");
    for entry in fs::read_dir(&dir).expect("sessions dir") {
        let path = entry.expect("entry").path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".template") else {
            continue;
        };
        let mut text = fs::read_to_string(&path).expect("read template");
        let mut out_name = stem.to_string();
        for (k, v) in &subs {
            text = text.replace(k, v);
            out_name = out_name.replace(k, v);
        }
        fs::write(dir.join(out_name), text).expect("write session json");
        fs::remove_file(&path).expect("remove template");
    }
    pids
}
