use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::locate::{self, Roots};
use crate::pty::{self, HolderConn};
use crate::screen::Screen;

const USAGE: &str = "用法：ccrpc [--toh-state-dir DIR] [--claude-config-dir DIR] <命令>
  ccrpc new [--cwd DIR] [--size COLSxROWS] [--co-display] [--log FILE] [-- claude 参数…]
  ccrpc attach [--force] [--co-display] [--log FILE] [--size COLSxROWS] <Holder ID | sessionId>
  ccrpc ls [--json]
  ccrpc plugin install | uninstall | generate <目录>
  ccrpc doctor
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
            "--toh-state-dir" => &mut roots.toh_state,
            "--claude-config-dir" => &mut roots.claude_config,
            _ => break,
        };
        let v = args
            .get(1)
            .with_context(|| format!("{a} 需要目录"))?
            .clone();
        *slot = std::path::PathBuf::from(v);
        args.drain(..2);
    }
    match args.first().map(String::as_str) {
        Some("hook") => {
            crate::hook::run_hook(&args[1..]);
            Ok(())
        }
        Some("plugin") => crate::plugin::cmd(&roots, &args[1..]),
        Some("ls") => cmd_ls(&roots, &args[1..]),
        Some("doctor") => cmd_doctor(&roots),
        Some("layouts") => cmd_layouts(&args[1..]),
        Some("debug") if args.get(1).map(String::as_str) == Some("replay") => cmd_replay(&args[2..]),
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
    let json_out = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => bail!("用法：ccrpc ls [--json]"),
    };
    let holders = locate::list_holders(roots);
    let table = locate::process_table()?;
    let state = crate::hook::state_dir();
    let mut rows = Vec::new();
    for (id, m) in &holders {
        let Some(claude) = locate::find_claude_under(&table, m.child_pid) else {
            continue;
        };
        let pf = crate::procfile::read(roots, claude.pid);
        let (sid, status) = match &pf {
            Some(p) => (
                p.session_id.clone().unwrap_or_default(),
                p.status.clone().unwrap_or_default(),
            ),
            None => (String::new(), "starting".into()),
        };
        let checkin = crate::hook::read_checkin(&state, claude.pid);
        let plugin = checkin.as_ref().is_some_and(|c| {
            crate::hook::checkin_problem(Some(c), pf.as_ref().and_then(|p| p.session_id.as_deref())).is_none()
        });
        let cwd = pf.as_ref().and_then(|p| p.cwd.clone()).unwrap_or_else(|| m.cwd.clone());
        if !json_out {
            println!("{id}\t{sid}\t{status}\t{}\t{}", m.cwd, if plugin { "plugin" } else { "-" });
            continue;
        }
        // The plugin's check-in, else where Claude Code puts it (`<projects>/<cwd encoded>/<id>.jsonl`).
        let guessed = (!sid.is_empty() && sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then(|| {
            let enc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            roots.projects_dir().join(enc).join(format!("{sid}.jsonl"))
        });
        let transcript = checkin
            .as_ref()
            .filter(|_| plugin)
            .map(|c| std::path::PathBuf::from(&c.transcript_path))
            .or(guessed)
            .filter(|p| locate::is_transcript_path(roots, p));
        rows.push(serde_json::json!({
            "holder_id": id,
            "session_id": (!sid.is_empty()).then_some(&sid),
            "status": status,
            "cwd": cwd,
            "plugin": plugin,
            "claude_pid": claude.pid,
            "version": pf.as_ref().and_then(|p| p.version.clone()),
            "title": transcript.as_deref().and_then(::transcript::assemble::skeleton::read_title),
        }));
    }
    if json_out {
        println!("{}", serde_json::Value::Array(rows));
    }
    Ok(())
}

/// PTY size of a Holder's Pane, from the tty of its shell.
pub fn pane_size(roots: &Roots, holder_id: &str) -> Result<(u16, u16)> {
    let meta = locate::read_holder(roots, holder_id)?;
    let table = locate::process_table()?;
    let tty = table
        .get(&meta.child_pid)
        .and_then(|p| p.tty.clone())
        .context("找不到 Pane shell 的 tty")?;
    locate::tty_size(&tty)
}

fn cmd_debug(roots: &Roots, args: &[String]) -> Result<()> {
    let (Some(sub), Some(id)) = (args.first(), args.get(1)) else {
        bail!("{USAGE}");
    };
    // Debug commands look and type without taking the PTY size; only `screen` needs the size.
    let (cols, rows) = match pane_size(roots, id) {
        Ok(s) => s,
        Err(e) if sub == "screen" => return Err(e),
        Err(_) => (80, 24),
    };
    let conn = HolderConn::connect(&roots.holder_socket(id), cols, rows)?;
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
        Ok(o) if o.status.success() => line("ok", "toh", String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => line("fail", "toh", "PATH 里没有可用的 toh".into()),
    }
    // claude
    match std::process::Command::new("claude").arg("--version").output() {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let ver = out.split_whitespace().next().unwrap_or_default().to_string();
            if crate::session::TESTED_CLAUDE_VERSIONS.contains(&ver.as_str()) {
                line("ok", "claude", out);
            } else {
                line("warn", "claude", format!("{out}：不是实测过的版本（{}），内置布局可能认不出", crate::session::TESTED_CLAUDE_VERSIONS.join(", ")));
            }
        }
        _ => line("fail", "claude", "PATH 里没有可用的 claude".into()),
    }
    // directories
    for (what, dir) in [("ToH 状态目录", &roots.toh_state), ("Claude Code 配置目录", &roots.claude_config)] {
        if dir.is_dir() {
            line("ok", what, dir.display().to_string());
        } else {
            line("fail", what, format!("{} 不存在", dir.display()));
        }
    }
    let sessions = roots.sessions_dir();
    match std::fs::read_dir(&sessions) {
        Ok(rd) => {
            let n = rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).count();
            line("ok", "进程文件", format!("{} 下 {n} 个", sessions.display()));
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
    // Holders
    let holders = locate::list_holders(roots);
    let table = locate::process_table()?;
    let with_claude = holders
        .iter()
        .filter(|(_, m)| locate::find_claude_under(&table, m.child_pid).is_some())
        .count();
    line("ok", "clay Holder", format!("{} 个，其中 {with_claude} 个在跑 Claude Code", holders.len()));
    // layouts and analyzer
    let lib = crate::layout::Library::load(crate::layout::library_dir());
    let learned = lib.layouts.iter().filter(|l| l.source == "learned").count();
    line(
        "ok",
        "布局库",
        format!("内置 {}、本机学到 {learned}（{}）", lib.layouts.len() - learned, lib.dir().display()),
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
        None => line("warn", "分析器", "没设 CCRPC_ANALYZER_API_KEY，认不出的界面只会报 unrecognized / dialog".into()),
    }
    if failed {
        bail!("有检查未通过");
    }
    Ok(())
}

/// `layouts list` / `layouts export <repo dir>`.
fn cmd_layouts(args: &[String]) -> Result<()> {
    let lib = crate::layout::Library::load(crate::layout::library_dir());
    match args.first().map(String::as_str) {
        Some("list") => {
            for l in &lib.layouts {
                let kind = serde_json::to_value(l.kind)?;
                println!("{}\t{}\t{}\t{}", l.id, l.source, kind.as_str().unwrap_or(""), l.title);
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
            eprintln!("导出 {n} 个布局；重新构建后成为内置布局，单测会用它们各自的屏幕检查。布局文件里有当时的整屏文字，提交前检查有没有不该进仓库的内容");
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
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
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
                        println!("{f}\tline {lineno}\tbranch at={at_uuid:?} dropped={}", live.len() - keep);
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
            println!("{f}\tmismatch live={} get_messages={}", live.len(), gm.len());
        }
        let _ = std::fs::remove_file(&p);
    }
    drop(tmpdir);
    println!("files={} branches={branches} mismatches={mismatches}", files.len());
    if mismatches > 0 {
        bail!("{mismatches} 个文件的实时视图与 get_messages 不一致");
    }
    Ok(())
}
