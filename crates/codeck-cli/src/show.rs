//! `show`: a Session (or Subagent Run) as compact text, one paragraph per turn.

use std::fmt::Write;

use codeck_core::model::{
    SessionDetail, ToolDetailRequest, ToolPart, TranscriptRequest, TranscriptScope,
};
use codeck_core::{CoreError, Engine};
use transcript::live;
use transcript::model::{AssistantBlock, NodeBody, PromptOrigin, ToolCall, Transcript};

use crate::args::Parsed;
use crate::common::{self, CliError, Context, cap, one_line};
use crate::turns::{self, Turn};

/// Without `--turn`: the newest turns, each cut short; the point is to find the turn to read.
const OVERVIEW_TURNS: usize = 50;
const OVERVIEW_PROMPT_CHARS: usize = 300;
const OVERVIEW_ANSWER_CHARS: usize = 600;
const OVERVIEW_TOOLS: usize = 12;
/// With `--turn`.
const TURN_TEXT_CHARS: usize = 20_000;
const TURN_TOOLS: usize = 200;
const TOOL_TARGET_CHARS: usize = 80;
/// `--full`: each tool input or output, and what all of a turn's together may reach.
const FULL_PART_CHARS: usize = 8_000;
const FULL_TOTAL_CHARS: usize = 200_000;
/// Hops of a Continued Session chain followed before giving up (a cycle guard).
const MAX_SUPERSEDED_HOPS: usize = 16;

pub fn run(cx: &Context, a: &Parsed) -> Result<String, CliError> {
    let id = a.one_positional("Session ID")?;
    // Goes into a path when the index does not know the Session.
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(CliError::usage("Session ID 不对"));
    }
    let wanted_turn = a.number("--turn", None)?;
    let full = a.flag("--full");
    if full && wanted_turn.is_none() {
        return Err(CliError::usage("--full 需要同时给 --turn"));
    }
    let engine = cx.engine()?;
    let root = cx.data_root().path;
    let (id, detail) = resolve(&engine, id)?;
    let scope = match a.value("--agent") {
        Some(agent_id) => TranscriptScope::Subagent {
            agent_id: agent_id.to_owned(),
        },
        None => TranscriptScope::Main,
    };
    let t = engine.get_transcript(&TranscriptRequest {
        session_id: id.clone(),
        scope: scope.clone(),
        branch_choices: Vec::new(),
        include_hidden: true,
    })?;
    let all = turns::split(&t);
    let is_agent = matches!(scope, TranscriptScope::Subagent { .. });

    let mut out = header(&engine, &root, &id, detail.as_ref(), &t, &all);
    match wanted_turn {
        Some(n) => {
            let Some(turn) = all.iter().find(|x| u64::from(x.number) == n) else {
                return Err(CliError::new(
                    "not_found",
                    format!("没有第 {n} 轮，共 {} 轮", real_turns(&all)),
                ));
            };
            write_turn(&mut out, turn, is_agent, true);
            if full {
                write_full(&mut out, &engine, &id, &scope, turn);
            }
        }
        None => {
            let skipped = all.len().saturating_sub(OVERVIEW_TURNS);
            if skipped > 0 {
                let _ = writeln!(
                    out,
                    "（只显示最后 {OVERVIEW_TURNS} 轮，更早的 {skipped} 轮用 --turn 查看）\n"
                );
            }
            // Before the first prompt there is usually nothing to say.
            for turn in all[skipped..]
                .iter()
                .filter(|t| t.number > 0 || !tool_calls(t).is_empty() || answer(t).is_some())
            {
                write_turn(&mut out, turn, is_agent, false);
            }
        }
    }
    Ok(out)
}

/// The Session to read: follows Continued Sessions, as the app does when opening one by id.
/// `None` when the index does not know it; the Transcript then comes from its file alone.
fn resolve(engine: &Engine, id: &str) -> Result<(String, Option<SessionDetail>), CliError> {
    let mut id = id.to_owned();
    for _ in 0..MAX_SUPERSEDED_HOPS {
        let detail = match engine.get_session(&id) {
            Ok(d) => d,
            Err(CoreError::NotFound(_)) => return Ok((id, None)),
            Err(e) => return Err(e.into()),
        };
        match detail.superseded_by.clone() {
            Some(next) if next != id => id = next,
            _ => return Ok((id, Some(detail))),
        }
    }
    Err(CliError::new("internal", "Session 的接续关系有环"))
}

fn real_turns(all: &[Turn<'_>]) -> usize {
    all.iter().filter(|t| t.number > 0).count()
}

fn header(
    engine: &Engine,
    root: &std::path::Path,
    id: &str,
    detail: Option<&SessionDetail>,
    t: &Transcript,
    all: &[Turn<'_>],
) -> String {
    let status = match detail {
        Some(d) => common::status_word(d.summary.live.as_ref()),
        None => {
            let pfs = live::read_process_files(&codeck_core::paths::sessions_dir(root));
            common::status_word(live::detect(&pfs).get(id))
        }
    };
    let mut line = format!("session {id}  status={status}  turns={}", real_turns(all));
    if let Some(d) = detail {
        let _ = write!(line, "  project={}", d.project_path);
    }
    if let TranscriptScope::Subagent { agent_id } = &t.scope {
        let _ = write!(line, "  agent={agent_id}");
    } else if let Some(range) = &t.inherited {
        let _ = write!(line, "  forkOf={}", range.origin_session_id);
    }
    if let Some(at) = engine.indexed_at() {
        let _ = write!(line, "  indexedAt={}", common::time_of(at));
    }
    if detail.is_none() {
        line.push_str("  partial");
    }
    line.push_str("\n\n");
    line
}

fn prompt_text(turn: &Turn<'_>) -> Option<String> {
    let NodeBody::UserPrompt {
        text,
        images,
        origin,
    } = &turn.prompt?.body
    else {
        return None;
    };
    let mut out = match origin {
        PromptOrigin::Command { name, args } if name == "!" => format!("!{args}"),
        PromptOrigin::Command { name, args } if args.is_empty() => name.clone(),
        PromptOrigin::Command { name, args } => format!("{name} {args}"),
        _ => text.clone(),
    };
    if !images.is_empty() {
        let _ = write!(out, " [图片 ×{}]", images.len());
    }
    Some(out)
}

fn tool_calls<'a>(turn: &'a Turn<'_>) -> Vec<&'a ToolCall> {
    turn.nodes
        .iter()
        .filter(|n| !n.hidden)
        .filter_map(|n| match &n.body {
            NodeBody::Assistant { blocks, .. } => Some(blocks),
            _ => None,
        })
        .flatten()
        .filter_map(|b| match b {
            AssistantBlock::ToolCall(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// The text after the turn's last tool call or thinking: what the viewer calls its answer.
fn answer(turn: &Turn<'_>) -> Option<String> {
    let mut texts: Vec<&str> = Vec::new();
    for n in turn.nodes.iter().filter(|n| !n.hidden) {
        let NodeBody::Assistant { blocks, .. } = &n.body else {
            continue;
        };
        for b in blocks {
            match b {
                AssistantBlock::Text { text } => texts.push(text),
                AssistantBlock::ToolCall(_) | AssistantBlock::Thinking { .. } => texts.clear(),
                AssistantBlock::Unknown { .. } => {}
            }
        }
    }
    let joined = texts.join("\n\n");
    (!joined.trim().is_empty()).then_some(joined)
}

fn write_turn(out: &mut String, turn: &Turn<'_>, is_agent: bool, whole: bool) {
    let label = match (is_agent, turn.number) {
        (true, 1) => "任务",
        (_, 0) => "（第一条提示词之前）",
        _ => "用户",
    };
    let (text_cap, tool_limit) = if whole {
        (TURN_TEXT_CHARS, TURN_TOOLS)
    } else {
        (OVERVIEW_PROMPT_CHARS, OVERVIEW_TOOLS)
    };
    let shape = |s: &str, max| {
        if whole {
            cap(s, max).replace('\n', "\n    ")
        } else {
            cap(&one_line(s), max)
        }
    };
    let prompt = prompt_text(turn).map_or(String::new(), |p| shape(&p, text_cap));
    let _ = writeln!(out, "[{}] {label}：{prompt}", turn.number);
    if whole {
        write_process(out, turn, &shape);
    } else {
        let calls = tool_calls(turn);
        if !calls.is_empty() {
            let mut parts: Vec<String> =
                calls.iter().take(tool_limit).map(|c| describe(c)).collect();
            if calls.len() > tool_limit {
                parts.push(format!("等 {} 次工具调用", calls.len() - tool_limit));
            }
            let _ = writeln!(out, "    工具：{}", parts.join("；"));
        }
    }
    let answer_cap = if whole {
        TURN_TEXT_CHARS
    } else {
        OVERVIEW_ANSWER_CHARS
    };
    if let Some(a) = answer(turn) {
        let _ = writeln!(out, "    回答：{}", shape(&a, answer_cap));
    }
    out.push('\n');
}

/// `--turn`: what led to the answer, in order, so a search hit in it can be found here.
fn write_process(out: &mut String, turn: &Turn<'_>, shape: &dyn Fn(&str, usize) -> String) {
    let blocks: Vec<&AssistantBlock> = turn
        .nodes
        .iter()
        .filter(|n| !n.hidden)
        .filter_map(|n| match &n.body {
            NodeBody::Assistant { blocks, .. } => Some(blocks),
            _ => None,
        })
        .flatten()
        .collect();
    let last_work = blocks.iter().rposition(|b| {
        matches!(
            b,
            AssistantBlock::ToolCall(_) | AssistantBlock::Thinking { .. }
        )
    });
    let Some(last_work) = last_work else {
        return;
    };
    let mut tools = 0;
    for b in &blocks[..=last_work] {
        match b {
            AssistantBlock::Text { text } if !text.trim().is_empty() => {
                let _ = writeln!(out, "    说明：{}", shape(text, TURN_TEXT_CHARS));
            }
            AssistantBlock::Thinking { text } if !text.trim().is_empty() => {
                let _ = writeln!(out, "    思考：{}", shape(text, TURN_TEXT_CHARS));
            }
            AssistantBlock::ToolCall(c) => {
                tools += 1;
                if tools <= TURN_TOOLS {
                    let _ = writeln!(out, "    工具：{}", describe(c));
                }
            }
            _ => {}
        }
    }
    if tools > TURN_TOOLS {
        let _ = writeln!(out, "    工具：等 {} 次工具调用", tools - TURN_TOOLS);
    }
}

/// Action and target, as the viewer's tool rows.
fn describe(c: &ToolCall) -> String {
    let input: serde_json::Value = serde_json::from_str(&c.input_json).unwrap_or_default();
    let field = |k: &str| -> String {
        let v = input.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let v = one_line(v);
        match v.char_indices().nth(TOOL_TARGET_CHARS) {
            Some((end, _)) => format!("{}…", &v[..end]),
            None => v,
        }
    };
    let mut s = match c.name.as_str() {
        "Read" => format!("读取 {}", field("file_path")),
        "Edit" | "MultiEdit" => format!("编辑 {}", field("file_path")),
        "NotebookEdit" => format!("编辑 {}", field("notebook_path")),
        "Write" => format!("写入 {}", field("file_path")),
        "Bash" => format!("运行 {}", field("command")),
        "Grep" | "Glob" => format!("搜索 {}", field("pattern")),
        "WebFetch" => format!("抓取 {}", field("url")),
        "WebSearch" => format!("网络搜索 {}", field("query")),
        "Agent" | "Task" => match &c.subagent_id {
            Some(id) => format!("Subagent {}（--agent {id}）", field("description")),
            None => format!("Subagent {}", field("description")),
        },
        other => other.to_owned(),
    };
    if c.result.as_ref().is_some_and(|r| r.is_error) {
        s.push_str("（失败）");
    }
    s
}

fn write_full(
    out: &mut String,
    engine: &Engine,
    id: &str,
    scope: &TranscriptScope,
    turn: &Turn<'_>,
) {
    let calls = tool_calls(turn);
    let budget_start = out.len();
    for (i, c) in calls.iter().enumerate() {
        if out.len() - budget_start > FULL_TOTAL_CHARS {
            let _ = writeln!(
                out,
                "（已达输出上限，省略其余 {} 次工具调用）",
                calls.len() - i
            );
            return;
        }
        let _ = writeln!(
            out,
            "--- 工具 {}：{}（{}）",
            i + 1,
            describe(c),
            c.tool_use_id
        );
        for (part, label) in [(ToolPart::Input, "输入"), (ToolPart::Output, "输出")] {
            let body = match engine.get_tool_detail(&ToolDetailRequest {
                session_id: id.to_owned(),
                scope: scope.clone(),
                tool_use_id: c.tool_use_id.clone(),
                part,
            }) {
                Ok(d) => {
                    let mut body = cap(&d.text, FULL_PART_CHARS);
                    if d.truncated {
                        let _ = write!(body, "…（原文共 {} 字节）", d.total_bytes as u64);
                    }
                    body
                }
                Err(_) => "（取不到）".to_owned(),
            };
            let _ = writeln!(out, "{label}：\n{body}");
        }
        out.push('\n');
    }
}
