use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::locate::{self, Roots};
use crate::pty::{self, HolderConn};
use crate::screen::Screen;

const USAGE: &str = "用法：ccrpc [--toh-state-dir DIR] [--claude-config-dir DIR] <命令>
  ccrpc new [--cwd DIR] [--size COLSxROWS] [-- claude 参数…]
  ccrpc attach [--force] [--size COLSxROWS] <Holder ID | sessionId>
  ccrpc ls
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
        Some("ls") => cmd_ls(&roots),
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

fn cmd_ls(roots: &Roots) -> Result<()> {
    let holders = locate::list_holders(roots);
    let table = locate::process_table()?;
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
        println!("{id}\t{sid}\t{status}\t{}", m.cwd);
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
