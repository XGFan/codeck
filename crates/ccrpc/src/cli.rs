use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::RecursiveMode;
use serde_json::{Value, json};

use crate::locate::{self, Roots};
use crate::pty::{self, HolderConn};
use crate::screen::Screen;
use crate::toh::Pane;
use crate::wake::{FsWatch, Wake};

const USAGE: &str = "用法：ccrpc [--toh-state-dir DIR] [--claude-config-dir DIR] <命令>
  ccrpc new [--cwd DIR] [--size COLSxROWS] [--hidden] [--log FILE] [-- claude 参数…]
  ccrpc new … --fork <sessionId> [--fork-at <uuid>] [--fork-restore conversation|both] [--fork-title 标题] [-- claude 参数…]
  ccrpc attach [--log FILE] <Holder ID | sessionId>
  ccrpc ls [--json [--watch]]
  ccrpc hide | show <Holder ID>
  ccrpc plugin install | uninstall | generate <目录>
  ccrpc doctor
  ccrpc analyzer test
  ccrpc layouts list
  ccrpc layouts export <仓库目录>
  ccrpc debug replay <transcript.jsonl>…
  ccrpc debug screen <Holder ID>
  ccrpc debug keys <Holder ID> <按键|text:文字>…
  ccrpc debug paste <Holder ID> <文字>";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut roots = Roots::from_env();
    // Global options before the subcommand override the environment.
    let mut args = args;
    while let Some(a) = args.first().cloned() {
        let slot = match a.as_str() {
            "--toh-state-dir" | "--claude-config-dir" => a.clone(),
            _ => break,
        };
        let v = PathBuf::from(
            args.get(1)
                .with_context(|| format!("{a} 需要目录"))?
                .clone(),
        );
        if slot == "--toh-state-dir" {
            roots.toh_state = Some(v);
        } else {
            roots.claude_config = v;
        }
        args.drain(..2);
    }
    match args.first().map(String::as_str) {
        Some("hook") => {
            crate::hook::run_hook(&args[1..]);
            Ok(())
        }
        Some("plugin") => crate::plugin::cmd(&roots, &args[1..]),
        Some("ls") => cmd_ls(&roots, &args[1..]),
        Some(c @ ("hide" | "show")) => cmd_place(&roots, c, &args[1..]),
        Some("doctor") => cmd_doctor(&roots),
        Some("analyzer") => cmd_analyzer(&args[1..]),
        Some("layouts") => cmd_layouts(&args[1..]),
        Some("debug") if args.get(1).map(String::as_str) == Some("replay") => {
            cmd_replay(&args[2..])
        }
        Some("debug") => cmd_debug(&roots, &args[1..]),
        Some("new") => crate::session::cmd_new(&roots, &args[1..]),
        Some("attach") => crate::session::cmd_attach(&roots, &args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn cmd_ls(roots: &Roots, args: &[String]) -> Result<()> {
    let (mut json_out, mut watch) = (false, false);
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            "--watch" => watch = true,
            _ => bail!("用法：ccrpc ls [--json [--watch]]"),
        }
    }
    if watch && !json_out {
        bail!("ls：--watch 只能和 --json 一起用");
    }
    if watch {
        return ls_watch(roots);
    }
    let panes = crate::toh::list(roots)?;
    let rows = ls_rows(roots, &panes, &mut LsState::default())?;
    if json_out {
        println!("{}", Value::Array(rows));
    } else {
        for r in &rows {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
            let id = s("holder_id");
            let pane_cwd = panes
                .iter()
                .find(|p| p.id == id)
                .map_or("", |p| p.cwd.as_str());
            println!(
                "{id}\t{}\t{}\t{pane_cwd}\t{}",
                s("session_id"),
                s("status"),
                if r["plugin"] == true { "plugin" } else { "-" }
            );
        }
    }
    Ok(())
}

/// What `ls` keeps from one list to the next: the `claude` under each Pane's shell (`ps` runs
/// again only when Panes or Process Files come or go, or one of those claude processes is gone),
/// and each transcript's title, read on from where it was last read.
#[derive(Default)]
struct LsState {
    /// The Panes' shells and the Process Files the map below was made for.
    key: Option<(Vec<u32>, Vec<std::ffi::OsString>)>,
    claude: HashMap<u32, locate::Proc>,
    titles: HashMap<PathBuf, ::transcript::assemble::skeleton::TitleTail>,
}

/// One row per Pane running Claude Code.
fn ls_rows(roots: &Roots, panes: &[Pane], st: &mut LsState) -> Result<Vec<Value>> {
    let mut files: Vec<std::ffi::OsString> = std::fs::read_dir(roots.sessions_dir())
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    files.sort();
    let key = (panes.iter().map(|p| p.shell_pid).collect::<Vec<_>>(), files);
    let gone = st
        .claude
        .values()
        .any(|c| !::transcript::live::pid_exists(c.pid));
    if st.key.as_ref() != Some(&key) || gone {
        let table = locate::process_table()?;
        st.claude = panes
            .iter()
            .filter_map(|p| {
                locate::find_claude_under(&table, p.shell_pid).map(|c| (p.shell_pid, c))
            })
            .collect();
        st.key = Some(key);
    }
    let state = crate::hook::state_dir();
    let mut rows = Vec::new();
    for pane in panes {
        let Some(claude) = st.claude.get(&pane.shell_pid) else {
            continue;
        };
        let (mut pf, mut checkin) = (None, crate::hook::read_checkin(&state, claude.pid));
        if let Some((p, job)) = crate::procfile::read_with_job(roots, claude.pid) {
            pf = Some(p);
            // Its conversation moved to the background: the Session is the one running there.
            if let locate::Moved::In { pf: b, checkin: c } =
                locate::moved(roots, &state, job.parked_job_id.as_deref())
            {
                (pf, checkin) = (Some(b), Some(c));
            }
        }
        let (sid, status) = match &pf {
            Some(p) => (
                p.session_id.clone().unwrap_or_default(),
                p.status.clone().unwrap_or_default(),
            ),
            None => (String::new(), "starting".into()),
        };
        let plugin = checkin.as_ref().is_some_and(|c| {
            crate::hook::checkin_problem(Some(c), pf.as_ref().and_then(|p| p.session_id.as_deref()))
                .is_none()
        });
        let cwd = pf
            .as_ref()
            .and_then(|p| p.cwd.clone())
            .unwrap_or_else(|| pane.cwd.clone());
        // The plugin's check-in, else where Claude Code puts it (`<projects>/<cwd encoded>/<id>.jsonl`).
        let guessed = (!sid.is_empty()
            && sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| {
            let enc: String = cwd
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            roots.projects_dir().join(enc).join(format!("{sid}.jsonl"))
        });
        let transcript = checkin
            .as_ref()
            .filter(|_| plugin)
            .map(|c| PathBuf::from(&c.transcript_path))
            .or(guessed)
            .filter(|p| locate::is_transcript_path(roots, p));
        let title = transcript
            .map(|t| st.titles.entry(t.clone()).or_default().read(&t))
            .unwrap_or_default();
        rows.push(json!({
            "holder_id": pane.id,
            "session_id": (!sid.is_empty()).then_some(&sid),
            "status": status,
            "cwd": cwd,
            "plugin": plugin,
            "hidden": pane.hidden,
            "claude_pid": claude.pid,
            "version": pf.as_ref().and_then(|p| p.version.clone()),
            "title": title,
        }));
    }
    Ok(rows)
}

const LS_BACKSTOP: Duration = Duration::from_secs(30);

/// A directory to watch for `dir`: itself, or the closest parent that exists (its creation then
/// wakes the loop, which watches it from then on).
fn watch_point(dir: &Path) -> &Path {
    let mut d = dir;
    while !d.exists()
        && let Some(p) = d.parent()
    {
        d = p;
    }
    d
}

/// `ls --json --watch`: the list once, then again each time it changes, until stdin closes.
/// Woken by `toh ls --json --watch` (Panes), and by file events on the Process Files and the
/// plugin's check-ins; a look every [`LS_BACKSTOP`] catches a claude that died without removing
/// its Process File.
fn ls_watch(roots: &Roots) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    let wake = Wake::new();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (stop, wake) = (stop.clone(), wake.clone());
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            stop.store(true, Ordering::SeqCst);
            wake.bump();
        });
    }
    let panes = crate::toh::Watch::spawn(roots, wake.clone())?;
    let mut fs = FsWatch::new(wake.clone());
    let sessions = roots.sessions_dir();
    let checkins = crate::hook::state_dir().join("sessions");
    let mut st = LsState::default();
    let mut last = None;
    loop {
        let seen = wake.seen();
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(why) = panes.ended() {
            bail!(why);
        }
        fs.set(&[
            (watch_point(&sessions), RecursiveMode::NonRecursive),
            (watch_point(&checkins), RecursiveMode::NonRecursive),
        ]);
        if let Some(list) = panes.latest() {
            let line = Value::Array(ls_rows(roots, &list, &mut st)?).to_string();
            if last.as_ref() != Some(&line) {
                let mut out = std::io::stdout().lock();
                if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                    return Ok(());
                }
                last = Some(line);
            }
        }
        wake.wait(seen, LS_BACKSTOP);
    }
}

/// `ccrpc hide|show <Holder ID>`: `toh hide|show`, hiding the Pane on the Mac or showing it again (ToH ADR-0050).
fn cmd_place(roots: &Roots, verb: &str, args: &[String]) -> Result<()> {
    let [id] = args else {
        bail!("用法：ccrpc {verb} <Holder ID>");
    };
    if !locate::valid_holder_id(id) {
        bail!("不是 Holder ID：{id}");
    }
    crate::toh::place(roots, verb, id)
}

fn cmd_debug(roots: &Roots, args: &[String]) -> Result<()> {
    let (Some(sub), Some(id)) = (args.first(), args.get(1)) else {
        bail!("{USAGE}");
    };
    // Debug commands look and type without changing the PTY size.
    let conn = HolderConn::connect(&crate::toh::pane(roots, id)?.socket)?;
    match sub.as_str() {
        "screen" => {
            let s: Screen = conn.resync(Duration::from_secs(3))?;
            for (i, l) in s.lines.iter().enumerate() {
                println!("{i:3}|{l}");
            }
            println!("cursor={:?}", s.cursor);
        }
        "keys" => {
            for k in &args[2..] {
                if let Some(t) = k.strip_prefix("text:") {
                    conn.input(t.as_bytes())?;
                } else {
                    let b = pty::key_bytes(k).with_context(|| format!("不认识的按键 {k}"))?;
                    conn.input(b)?;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        "paste" => {
            conn.paste(&args[2..].join(" "))?;
        }
        _ => bail!("{USAGE}"),
    }
    std::thread::sleep(Duration::from_millis(100));
    conn.detach();
    Ok(())
}

/// Checks what ccrpc depends on and prints one line per check (`ok` / `warn` / `fail`).
fn cmd_doctor(roots: &Roots) -> Result<()> {
    let mut failed = false;
    let mut line = |level: &str, what: &str, detail: String| {
        if level == "fail" {
            failed = true;
        }
        println!("{level}\t{what}\t{detail}");
    };
    // toh
    match std::process::Command::new("toh").arg("--version").output() {
        Ok(o) if o.status.success() => line(
            "ok",
            "toh",
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        ),
        _ => line("fail", "toh", "PATH 里没有可用的 toh".into()),
    }
    // claude
    match std::process::Command::new("claude")
        .arg("--version")
        .output()
    {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let ver = out
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            if crate::session::TESTED_CLAUDE_VERSIONS.contains(&ver.as_str()) {
                line("ok", "claude", out);
            } else {
                line(
                    "warn",
                    "claude",
                    format!(
                        "{out}：不是实测过的版本（{}），内置布局可能认不出",
                        crate::session::TESTED_CLAUDE_VERSIONS.join(", ")
                    ),
                );
            }
        }
        _ => line("fail", "claude", "PATH 里没有可用的 claude".into()),
    }
    // directories
    let dir = &roots.claude_config;
    if dir.is_dir() {
        line("ok", "Claude Code 配置目录", dir.display().to_string());
    } else {
        line(
            "fail",
            "Claude Code 配置目录",
            format!("{} 不存在", dir.display()),
        );
    }
    let sessions = roots.sessions_dir();
    match std::fs::read_dir(&sessions) {
        Ok(rd) => {
            let n = rd
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count();
            line(
                "ok",
                "进程文件",
                format!("{} 下 {n} 个", sessions.display()),
            );
        }
        Err(_) => line("warn", "进程文件", format!("读不到 {}", sessions.display())),
    }
    // TUI mode (read-only)
    let settings = roots.claude_config.join("settings.json");
    let tui = std::fs::read(&settings)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["tui"].as_str().map(str::to_string))
        .unwrap_or_else(|| "default".into());
    line("ok", "TUI", tui);
    for (level, detail) in crate::plugin::doctor(roots) {
        line(level, "ccrpc 插件", detail);
    }
    // Holders, as toh lists them
    match crate::toh::list(roots) {
        Ok(panes) => {
            let table = locate::process_table()?;
            let with_claude = panes
                .iter()
                .filter(|p| locate::find_claude_under(&table, p.shell_pid).is_some())
                .count();
            line(
                "ok",
                "Holder",
                format!("{} 个，其中 {with_claude} 个在跑 Claude Code", panes.len()),
            );
        }
        Err(e) => line("fail", "Holder", format!("{e:#}")),
    }
    // layouts and analyzer
    let lib = crate::layout::Library::load(crate::layout::library_dir());
    let learned = lib.layouts.iter().filter(|l| l.source == "learned").count();
    line(
        "ok",
        "布局库",
        format!(
            "内置 {}、本机学到 {learned}（{}）",
            lib.layouts.len() - learned,
            lib.dir().display()
        ),
    );
    match crate::analyze::Analyzer::from_env() {
        Some(a) => {
            // Credentials written into the URL are not printed.
            let shown = match a.base_url.split_once("://") {
                Some((scheme, rest)) => match rest.split_once('@') {
                    Some((_, host)) => format!("{scheme}://{host}"),
                    None => a.base_url.clone(),
                },
                None => a.base_url.clone(),
            };
            line("ok", "分析器", format!("{shown} · {}", a.model))
        }
        None => line(
            "warn",
            "分析器",
            "没设 CCRPC_ANALYZER_API_KEY，认不出的界面只会报 unrecognized / dialog".into(),
        ),
    }
    if failed {
        bail!("有检查未通过");
    }
    Ok(())
}

/// `analyzer test`: one analysis of a built-in dialog with the analyzer the environment sets; prints
/// `{"model", "elapsed_ms", "kind", "options"}` on success.
fn cmd_analyzer(args: &[String]) -> Result<()> {
    if !matches!(args, [a] if a == "test") {
        bail!("用法：ccrpc analyzer test");
    }
    let a = crate::analyze::Analyzer::from_env().context("没设 CCRPC_ANALYZER_API_KEY")?;
    let (layout, elapsed) = a.test()?;
    println!(
        "{}",
        serde_json::json!({
            "model": a.model,
            "elapsed_ms": elapsed.as_millis() as u64,
            "kind": layout.kind,
            "options": layout.options.len(),
        })
    );
    Ok(())
}

/// `layouts list` / `layouts export <repo dir>`.
fn cmd_layouts(args: &[String]) -> Result<()> {
    let lib = crate::layout::Library::load(crate::layout::library_dir());
    match args.first().map(String::as_str) {
        Some("list") => {
            for l in &lib.layouts {
                let kind = serde_json::to_value(l.kind)?;
                println!(
                    "{}\t{}\t{}\t{}",
                    l.id,
                    l.source,
                    kind.as_str().unwrap_or(""),
                    l.title
                );
            }
            Ok(())
        }
        Some("export") => {
            let Some(repo) = args.get(1) else {
                bail!("用法：ccrpc layouts export <仓库目录>");
            };
            let dir = std::path::Path::new(repo).join("layouts");
            std::fs::create_dir_all(&dir).with_context(|| format!("建不了 {}", dir.display()))?;
            let mut n = 0;
            for l in lib.layouts.iter().filter(|l| l.source == "learned") {
                let mut out = l.clone();
                out.source = String::new();
                let path = dir.join(format!("{}.json", l.id));
                std::fs::write(&path, serde_json::to_string_pretty(&out)? + "\n")?;
                println!("{}", path.display());
                n += 1;
            }
            eprintln!(
                "导出 {n} 个布局；重新构建后成为内置布局，单测会用它们各自的屏幕检查。布局文件里有当时的整屏文字，提交前检查有没有不该进仓库的内容"
            );
            Ok(())
        }
        _ => bail!("用法：ccrpc layouts list | ccrpc layouts export <仓库目录>"),
    }
}

/// Replays transcripts line by line through the tail and checks that the live view (blocks minus
/// what each `branch` drops) equals `get_messages`; prints each Branch. Fails on a mismatch.
fn cmd_replay(files: &[String]) -> Result<()> {
    use crate::transcript::{Tailer, TxEvent, main_blocks};
    use std::io::Write;
    if files.is_empty() {
        bail!("用法：ccrpc debug replay <transcript.jsonl>…");
    }
    // A fresh private directory (random name, owner-only, removed on drop).
    let tmpdir = tempfile::Builder::new().prefix("ccrpc-replay-").tempdir()?;
    let tmp = tmpdir.path().to_path_buf();
    let (mut branches, mut mismatches) = (0usize, 0usize);
    for (n, f) in files.iter().enumerate() {
        let Ok(data) = std::fs::read(f) else {
            eprintln!("读不了 {f}");
            continue;
        };
        let p = tmp.join(format!("t{n}.jsonl"));
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)?;
        let mut t = Tailer::new(&p);
        let mut live: Vec<String> = Vec::new();
        let mut apply = |evs: Vec<TxEvent>, live: &mut Vec<String>, lineno: usize| {
            for ev in evs {
                match ev {
                    TxEvent::Block(b) if b.agent_id.is_none() => {
                        if live.last() != Some(&b.uuid) {
                            live.push(b.uuid.clone());
                        }
                    }
                    TxEvent::Branch { at_uuid } => {
                        branches += 1;
                        let keep = at_uuid
                            .as_ref()
                            .and_then(|a| live.iter().rposition(|u| u == a))
                            .map_or(0, |i| i + 1);
                        println!(
                            "{f}\tline {lineno}\tbranch at={at_uuid:?} dropped={}",
                            live.len() - keep
                        );
                        live.truncate(keep);
                    }
                    _ => {}
                }
            }
        };
        for (i, line) in data.split(|&b| b == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            out.write_all(line)?;
            out.write_all(b"\n")?;
            out.flush()?;
            loop {
                let evs = t.poll();
                apply(evs, &mut live, i + 1);
                if !t.behind() {
                    break;
                }
            }
        }
        let (blocks, _) = main_blocks(&p, 0);
        let mut gm: Vec<String> = Vec::new();
        for b in blocks {
            if gm.last() != Some(&b.uuid) {
                gm.push(b.uuid);
            }
        }
        if gm != live {
            mismatches += 1;
            println!(
                "{f}\tmismatch live={} get_messages={}",
                live.len(),
                gm.len()
            );
        }
        let _ = std::fs::remove_file(&p);
    }
    drop(tmpdir);
    println!(
        "files={} branches={branches} mismatches={mismatches}",
        files.len()
    );
    if mismatches > 0 {
        bail!("{mismatches} 个文件的实时视图与 get_messages 不一致");
    }
    Ok(())
}
