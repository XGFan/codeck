//! `codeck`: lets an AI (Claude through Bash, typically) read this machine's Sessions (ADR-0013).
//! The one write is `new`; everything else is read-only and the index is never rebuilt.
//!
//! stdout carries one JSON document (`show`: text), an error is one JSON line on stderr. Output
//! types are defined here and converted from the IPC types, which change with the interface.

mod args;
mod common;
pub mod new;
mod search;
mod sessions;
mod show;
mod turns;
mod wait;

use serde::Serialize;

use crate::args::Parsed;
use crate::common::CliError;

const USAGE: &str = "用法：codeck [--cache-dir DIR] <命令>
  codeck sessions [--project P] [--live] [--limit N]    列出 Session，带 Live 状态
  codeck search 词 [--project P] [--limit N]            全文搜索，语法同 ⌘K（\"短语\"、-排除词）
  codeck show <id> [--turn N] [--full] [--agent ID]     紧凑文本形式的对话，默认 Main Line
  codeck wait <id> [--timeout 秒]                       等到 Session 变成 idle、waiting 或结束，超时退出码 124
  codeck new --cwd DIR                                  新建 Session，提示词从 stdin 读
--project 收 Project 的路径，--full 需要 --turn。--cache-dir 是索引所在目录，默认桌面的。";

/// Runs the command line; returns the exit code.
pub fn run(args: Vec<String>) -> i32 {
    match dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}", serde_json::to_string(&e).unwrap_or_default());
            1
        }
    }
}

fn dispatch(args: Vec<String>) -> Result<i32, CliError> {
    let mut args = args;
    let cache_dir = args::take_cache_dir(&mut args)?;
    let Some((command, rest)) = args.split_first() else {
        return Err(CliError::usage("缺少命令"));
    };
    let cx = common::Context { cache_dir };
    match command.as_str() {
        "-h" | "--help" | "help" => {
            common::write_stdout(&format!("{USAGE}\n"));
            Ok(0)
        }
        "sessions" => {
            let a = Parsed::parse(rest, &["--project", "--limit"], &["--live"])?;
            print_json(&sessions::run(&cx, &a)?);
            Ok(0)
        }
        "search" => {
            let a = Parsed::parse(rest, &["--project", "--limit"], &[])?;
            print_json(&search::run(&cx, &a)?);
            Ok(0)
        }
        "show" => {
            let a = Parsed::parse(rest, &["--turn", "--agent"], &["--full"])?;
            common::write_stdout(&show::run(&cx, &a)?);
            Ok(0)
        }
        "wait" => {
            let a = Parsed::parse(rest, &["--timeout"], &[])?;
            let (output, code) = wait::run(&cx, &a)?;
            print_json(&output);
            Ok(code)
        }
        "new" => {
            let a = Parsed::parse(rest, &["--cwd"], &[])?;
            print_json(&new::run(&a)?);
            Ok(0)
        }
        other => Err(CliError::usage(format!("不认识的命令：{other}"))),
    }
}

pub(crate) fn print_json<T: Serialize>(value: &T) {
    let text = serde_json::to_string(value).unwrap_or_default();
    common::write_stdout(&format!("{text}\n"));
}
