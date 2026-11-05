//! ccrpc's interface to ToH (ToH ADR-0051, docs/api.md「本机机读接口」): `toh` subcommands that
//! print JSON, then the Holder's unix socket at the address they give. Nothing under ToH's state
//! directory is read or located here.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::locate::Roots;
use crate::wake::Wake;

/// One Holder, as `toh ls --json` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pane {
    pub id: String,
    #[serde(default)]
    pub backend: Option<String>,
    /// In clay's hidden Window (ToH ADR-0050); `None` outside clay.
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Where the Pane was opened.
    #[serde(default)]
    pub cwd: String,
    /// The Pane's shell (what `toh new` runs its command under).
    pub shell_pid: u32,
    pub socket: PathBuf,
}

/// `toh`, against ccrpc's ToH state directory when one is set, without the caller's terminal and
/// session variables.
pub fn command(roots: &Roots) -> Command {
    let mut cmd = Command::new("toh");
    if let Some(d) = &roots.toh_state {
        cmd.env("TOH_STATE_DIR", d);
    }
    for k in [
        "TOH_CONFIG",
        "KITTY_WINDOW_ID",
        "KITTY_LISTEN_ON",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "KAKU_UNIX_SOCKET",
        "ITERM_SESSION_ID",
        "TOH_HOLDER_ID",
        "CLAUDECODE",
        "CCRPC_ANALYZER_API_KEY",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

/// Runs `toh <args>` and returns its stdout.
fn run(roots: &Roots, args: &[&str]) -> Result<Vec<u8>> {
    let out = command(roots)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("运行 toh 失败（toh 在 PATH 里吗？）")?;
    if !out.status.success() {
        bail!(
            "toh {} 失败：{}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// An older `toh` prints its table for `ls --json`, or rejects the option.
fn parse<T: serde::de::DeserializeOwned>(what: &str, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .with_context(|| format!("toh {what} 的输出不是 JSON：toh 版本太旧，先更新 toh"))
}

/// Every live Holder (`toh ls --json`).
pub fn list(roots: &Roots) -> Result<Vec<Pane>> {
    parse("ls --json", &run(roots, &["ls", "--json"])?)
}

/// The live Holder `id`.
pub fn pane(roots: &Roots, id: &str) -> Result<Pane> {
    list(roots)?
        .into_iter()
        .find(|p| p.id == id)
        .with_context(|| format!("没有运行中的 Holder {id}"))
}

/// `toh new --json <args>`: the new Holder.
pub fn new_pane(roots: &Roots, args: &[String]) -> Result<Pane> {
    let mut all = vec!["new", "--json"];
    all.extend(args.iter().map(String::as_str));
    parse("new --json", &run(roots, &all)?)
}

/// `toh hide|show <id>`.
pub fn place(roots: &Roots, verb: &str, id: &str) -> Result<()> {
    run(roots, &[verb, id]).map(|_| ())
}

/// `toh ls --json --watch`: the latest list, kept by a reader thread that bumps `wake` on every
/// new one. The child ends when this is dropped (it also ends by itself once ccrpc exits).
pub struct Watch {
    child: Child,
    latest: Arc<Mutex<Option<Vec<Pane>>>>,
    ended: Arc<Mutex<Option<String>>>,
}

impl Watch {
    pub fn spawn(roots: &Roots, wake: Arc<Wake>) -> Result<Watch> {
        let mut child = command(roots)
            .args(["ls", "--json", "--watch"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("运行 toh 失败（toh 在 PATH 里吗？）")?;
        let stdout = child.stdout.take().context("toh 没有 stdout")?;
        let stderr = child.stderr.take();
        let latest = Arc::new(Mutex::new(None));
        let ended = Arc::new(Mutex::new(None));
        let (l, e) = (latest.clone(), ended.clone());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match parse::<Vec<Pane>>("ls --json --watch", line.as_bytes()) {
                    Ok(panes) => *l.lock().unwrap_or_else(|p| p.into_inner()) = Some(panes),
                    Err(err) => {
                        *e.lock().unwrap_or_else(|p| p.into_inner()) = Some(format!("{err:#}"));
                        wake.bump();
                        return;
                    }
                }
                wake.bump();
            }
            let why = stderr
                .and_then(|s| std::io::read_to_string(s).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "toh ls --json --watch 退出了".into());
            *e.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
            wake.bump();
        });
        Ok(Watch {
            child,
            latest,
            ended,
        })
    }

    /// The list last printed (`None` before the first).
    pub fn latest(&self) -> Option<Vec<Pane>> {
        self.latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Why the watch stopped, once it has.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
