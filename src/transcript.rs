//! Incremental tail of a Session's transcript (main file plus Subagent Run files) producing Blocks
//! and the few markers the state machine needs. Reads only complete lines (cv-core `parse`).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use cv_core::raw::{RawBlock, RawContent, RawEntry, decode_str};
use serde::Serialize;
use serde_json::Value;

/// Cap on text carried by one block event (tool outputs can be huge).
pub const MAX_BLOCK_TEXT: usize = 100 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    User,
    Text,
    Thinking,
    ToolUse,
    ToolResult,
}

/// One content unit of a transcript message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    pub kind: BlockKind,
    /// uuid of the transcript entry holding it.
    pub uuid: String,
    /// Position within the entry's content array.
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// tool_use id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// User input delivered mid-turn (typed while busy).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// Set when the block comes from a Subagent Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
}

impl Block {
    fn new(kind: BlockKind, uuid: &str, index: usize) -> Block {
        Block {
            kind,
            uuid: uuid.to_string(),
            index,
            text: None,
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            content: None,
            is_error: None,
            queued: false,
            agent_id: None,
            parent_tool_use_id: None,
        }
    }
}

/// What a poll found, in transcript order.
#[derive(Debug, Clone, PartialEq)]
pub enum TxEvent {
    Block(Block),
    /// `system` / `turn_duration` in the main file: the end of a turn.
    TurnEnd,
    /// `queue-operation` / `enqueue`: input typed while busy was accepted.
    Enqueued {
        content: String,
    },
    /// A user prompt entry in the main file (string content).
    UserPrompt {
        uuid: String,
        text: String,
    },
}

/// A tool_use that has no tool_result yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTool {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub agent_id: Option<String>,
    /// Arrival order.
    pub seq: u64,
}

#[derive(Debug)]
struct SubFile {
    path: PathBuf,
    offset: u64,
    parent_tool_use_id: Option<String>,
}

#[derive(Debug)]
pub struct Tailer {
    main: PathBuf,
    offset: u64,
    subs: HashMap<String, SubFile>,
    pending: HashMap<String, PendingTool>,
    seq: u64,
    last_text: Option<String>,
    /// Set only during the silent catch-up read in `new(_, true)`.
    from_end: bool,
}

/// Directory holding the Subagent Run files of `main` (`<main without .jsonl>/subagents`).
pub fn subagents_dir(main: &Path) -> PathBuf {
    main.with_extension("").join("subagents")
}

impl Tailer {
    /// Starts at the end of the existing files (`from_end`) or at the beginning.
    pub fn new(main: &Path, from_end: bool) -> Tailer {
        let mut t = Tailer {
            main: main.to_path_buf(),
            offset: 0,
            subs: HashMap::new(),
            pending: HashMap::new(),
            seq: 0,
            last_text: None,
            from_end,
        };
        if from_end {
            // Catch up silently so pending tools and the last text are known.
            let _ = t.poll();
        }
        t.from_end = false;
        t
    }

    pub fn main_path(&self) -> &Path {
        &self.main
    }

    /// Last assistant text block seen in the main file.
    pub fn last_text(&self) -> Option<&str> {
        self.last_text.as_deref()
    }

    /// Tools still waiting for a result, oldest first.
    pub fn pending(&self) -> Vec<&PendingTool> {
        let mut v: Vec<_> = self.pending.values().collect();
        v.sort_by_key(|p| p.seq);
        v
    }

    /// Oldest pending tool named `name` (any agent).
    pub fn pending_named(&self, name: &str) -> Option<&PendingTool> {
        self.pending().into_iter().find(|p| p.name == name)
    }

    /// Oldest pending tool other than AskUserQuestion.
    pub fn pending_for_permission(&self) -> Option<&PendingTool> {
        self.pending()
            .into_iter()
            .find(|p| p.name != "AskUserQuestion")
    }

    pub fn poll(&mut self) -> Vec<TxEvent> {
        let mut out = Vec::new();
        // A file that shrank was replaced: start it over.
        if std::fs::metadata(&self.main).map(|m| m.len()).unwrap_or(0) < self.offset {
            self.offset = 0;
        }
        if let Ok(chunk) = read_lines(&self.main, self.offset) {
            self.offset = chunk.consumed_to;
            for l in chunk.lines {
                self.on_entry(&l, None, None, &mut out);
            }
        }
        self.discover_subs();
        let mut ids: Vec<String> = self.subs.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let (path, offset, parent) = {
                let s = &self.subs[&id];
                (s.path.clone(), s.offset, s.parent_tool_use_id.clone())
            };
            if let Ok(chunk) = read_lines(&path, offset) {
                if let Some(s) = self.subs.get_mut(&id) {
                    s.offset = chunk.consumed_to;
                }
                for l in chunk.lines {
                    self.on_entry(&l, Some(&id), parent.as_deref(), &mut out);
                }
            }
        }
        if self.from_end {
            out.clear();
        }
        out
    }

    fn discover_subs(&mut self) {
        let dir = subagents_dir(&self.main);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".jsonl") else {
                continue;
            };
            let Some(agent_id) = stem.strip_prefix("agent-") else {
                continue;
            };
            if let Some(s) = self.subs.get_mut(agent_id) {
                if s.parent_tool_use_id.is_none() {
                    s.parent_tool_use_id = read_meta_tool_use_id(&dir, stem);
                }
                continue;
            }
            let offset = 0;
            self.subs.insert(
                agent_id.to_string(),
                SubFile {
                    path: p.clone(),
                    offset,
                    parent_tool_use_id: read_meta_tool_use_id(&dir, stem),
                },
            );
        }
    }

    fn on_entry(
        &mut self,
        line: &Line,
        agent: Option<&str>,
        parent: Option<&str>,
        out: &mut Vec<TxEvent>,
    ) {
        let e = &line.entry;
        let uuid = e.uuid.clone().unwrap_or_default();
        let tag = |mut b: Block| {
            b.agent_id = agent.map(str::to_string);
            b.parent_tool_use_id = parent.map(str::to_string);
            b
        };
        match e.entry_type() {
            "system" if agent.is_none() && e.subtype.as_deref() == Some("turn_duration") => {
                self.pending.retain(|_, p| p.agent_id.is_some());
                out.push(TxEvent::TurnEnd);
            }
            "queue-operation" if agent.is_none() => {
                if line.operation.as_deref() == Some("enqueue") {
                    let content = decode_str(e.content.as_deref()).unwrap_or_default();
                    out.push(TxEvent::Enqueued { content });
                }
            }
            "attachment" if agent.is_none() => {
                let Some(att) = e.attachment.as_deref() else {
                    return;
                };
                let Ok(v) = serde_json::from_str::<Value>(att.get()) else {
                    return;
                };
                if v.get("type").and_then(Value::as_str) != Some("queued_command") {
                    return;
                }
                if v.get("commandMode")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m != "prompt")
                {
                    return;
                }
                let text = match v.get("prompt") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => return,
                };
                let mut b = Block::new(BlockKind::User, &uuid, 0);
                b.text = Some(text);
                b.queued = true;
                out.push(TxEvent::Block(b));
            }
            "user" | "assistant" => {
                if e.is_meta == Some(true) {
                    return;
                }
                let assistant = e.entry_type() == "assistant";
                let Some(msg) = &e.message else {
                    return;
                };
                match &msg.content {
                    Some(RawContent::Text(s)) if !assistant => {
                        if is_command_record(s) {
                            return;
                        }
                        let mut b = Block::new(BlockKind::User, &uuid, 0);
                        b.text = Some(s.clone());
                        if agent.is_none() {
                            // A new prompt starts a new turn: its last text is not the previous one's.
                            self.last_text = None;
                            out.push(TxEvent::UserPrompt {
                                uuid: uuid.clone(),
                                text: s.clone(),
                            });
                        }
                        out.push(TxEvent::Block(tag(b)));
                    }
                    Some(RawContent::Text(s)) => {
                        let mut b = Block::new(BlockKind::Text, &uuid, 0);
                        b.text = Some(s.clone());
                        if agent.is_none() {
                            self.last_text = Some(s.clone());
                        }
                        out.push(TxEvent::Block(tag(b)));
                    }
                    Some(RawContent::Blocks(blocks)) => {
                        for (i, rb) in blocks.iter().enumerate() {
                            if let Some(b) = self.block_of(rb, &uuid, i, assistant, agent) {
                                out.push(TxEvent::Block(tag(b)));
                            }
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn block_of(
        &mut self,
        rb: &RawBlock,
        uuid: &str,
        i: usize,
        assistant: bool,
        agent: Option<&str>,
    ) -> Option<Block> {
        match rb.block_type() {
            "text" => {
                let t = rb.text_str()?;
                if assistant && agent.is_none() {
                    self.last_text = Some(t.clone());
                }
                let mut b = Block::new(
                    if assistant {
                        BlockKind::Text
                    } else {
                        BlockKind::User
                    },
                    uuid,
                    i,
                );
                b.text = Some(cap(t));
                Some(b)
            }
            "thinking" if assistant => {
                let mut b = Block::new(BlockKind::Thinking, uuid, i);
                b.text = Some(cap(rb.thinking_str().unwrap_or_default()));
                Some(b)
            }
            "tool_use" if assistant => {
                let id = rb.id.clone().unwrap_or_default();
                let name = rb.name.clone().unwrap_or_default();
                let input = rb
                    .input
                    .as_deref()
                    .and_then(|r| serde_json::from_str::<Value>(r.get()).ok())
                    .unwrap_or(Value::Null);
                self.seq += 1;
                self.pending.insert(
                    id.clone(),
                    PendingTool {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        agent_id: agent.map(str::to_string),
                        seq: self.seq,
                    },
                );
                let mut b = Block::new(BlockKind::ToolUse, uuid, i);
                b.id = Some(id);
                b.name = Some(name);
                b.input = Some(input);
                Some(b)
            }
            "tool_result" if !assistant => {
                let tid = rb.tool_use_id.clone().unwrap_or_default();
                self.pending.remove(&tid);
                let mut b = Block::new(BlockKind::ToolResult, uuid, i);
                b.tool_use_id = Some(tid);
                b.content = Some(cap(result_text(rb.content.as_deref())));
                b.is_error = Some(rb.is_error.unwrap_or(false));
                Some(b)
            }
            _ => None,
        }
    }
}

/// Every Block of the main file from the beginning (for `get_messages`).
pub fn main_blocks(main: &Path) -> Vec<Block> {
    let mut t = Tailer::new(main, false);
    let mut v = VecDeque::new();
    if let Ok(chunk) = read_lines(main, 0) {
        t.offset = chunk.consumed_to;
        let mut out = Vec::new();
        for l in chunk.lines {
            t.on_entry(&l, None, None, &mut out);
        }
        for ev in out {
            if let TxEvent::Block(b) = ev {
                v.push_back(b);
            }
        }
    }
    v.into()
}

fn read_meta_tool_use_id(dir: &Path, stem: &str) -> Option<String> {
    let bytes = std::fs::read(dir.join(format!("{stem}.meta.json"))).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("toolUseId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One complete transcript line: cv-core's tolerant entry plus the `operation` field of
/// `queue-operation` entries (which cv-core does not model).
struct Line {
    entry: RawEntry,
    operation: Option<String>,
}

#[derive(serde::Deserialize)]
struct OperationField {
    #[serde(default)]
    operation: Option<String>,
}

struct Chunk {
    lines: Vec<Line>,
    consumed_to: u64,
}

/// Reads the complete lines of `path` from byte `from` (read-only; a partial last line is left
/// for the next read; lines that are not JSON objects are skipped).
fn read_lines(path: &Path, from: u64) -> std::io::Result<Chunk> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len <= from {
        return Ok(Chunk {
            lines: Vec::new(),
            consumed_to: from.min(len),
        });
    }
    f.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    f.read_to_end(&mut buf)?;
    let end = match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => {
            return Ok(Chunk {
                lines: Vec::new(),
                consumed_to: from,
            });
        }
    };
    let mut lines = Vec::new();
    for raw in buf[..end].split(|&b| b == b'\n') {
        let raw = raw.trim_ascii();
        if raw.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<RawEntry>(raw) else {
            continue;
        };
        let operation = if entry.entry_type() == "queue-operation" {
            serde_json::from_slice::<OperationField>(raw)
                .ok()
                .and_then(|o| o.operation)
        } else {
            None
        };
        lines.push(Line { entry, operation });
    }
    Ok(Chunk {
        lines,
        consumed_to: from + end as u64,
    })
}

/// Slash-command bookkeeping that Claude Code records as user entries.
fn is_command_record(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("<local-command-caveat>")
        || t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
}

fn result_text(raw: Option<&serde_json::value::RawValue>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    match serde_json::from_str::<Value>(raw.get()) {
        Ok(Value::String(s)) => s,
        Ok(Value::Array(a)) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_BLOCK_TEXT {
        let mut end = MAX_BLOCK_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("…[truncated]");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transcripts")
            .join(name)
    }

    fn blocks(evs: &[TxEvent]) -> Vec<&Block> {
        evs.iter()
            .filter_map(|e| {
                if let TxEvent::Block(b) = e {
                    Some(b)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn reads_prompts_blocks_turn_ends_and_steer() {
        let mut t = Tailer::new(&fixture("steer_ask_permission.jsonl"), false);
        let evs = t.poll();
        let prompts: Vec<_> = evs
            .iter()
            .filter_map(|e| {
                if let TxEvent::UserPrompt { text, .. } = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            prompts[0],
            "Reply with exactly the word PONG and nothing else."
        );
        let turn_ends = evs.iter().filter(|e| matches!(e, TxEvent::TurnEnd)).count();
        assert!(turn_ends >= 5, "{turn_ends}");
        let bs = blocks(&evs);
        assert!(
            bs.iter()
                .any(|b| b.kind == BlockKind::Text && b.text.as_deref() == Some("PONG"))
        );
        let ask = bs
            .iter()
            .find(|b| b.name.as_deref() == Some("AskUserQuestion"))
            .unwrap();
        assert!(ask.input.as_ref().unwrap()["questions"].is_array());
        assert!(bs.iter().any(|b| {
            b.kind == BlockKind::ToolResult
                && b.content
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("Your questions have been answered")
        }));
        // steer: enqueue marker and the queued_command delivered as a queued user block
        assert!(
            evs.iter().any(
                |e| matches!(e, TxEvent::Enqueued { content } if content.starts_with("STEER:"))
            )
        );
        assert!(
            bs.iter()
                .any(|b| b.queued && b.text.as_deref().unwrap_or("").starts_with("STEER:"))
        );
        // every tool_use got its result, so nothing is pending at the end
        assert!(t.pending().is_empty(), "{:?}", t.pending());
        assert!(t.last_text().is_some());
    }

    #[test]
    fn subagent_blocks_are_tagged_and_command_records_skipped() {
        let mut t = Tailer::new(&fixture("subagent_deny.jsonl"), false);
        let evs = t.poll();
        let bs = blocks(&evs);
        assert!(
            !bs.iter()
                .any(|b| b.text.as_deref().unwrap_or("").contains("<command-name>"))
        );
        let sub: Vec<_> = bs.iter().filter(|b| b.agent_id.is_some()).collect();
        assert!(!sub.is_empty());
        assert!(
            sub.iter()
                .all(|b| b.agent_id.as_deref() == Some("a1f4e2d8aa4a5bb8d")
                    && b.parent_tool_use_id.as_deref()
                        == Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243"))
        );
        assert!(sub.iter().any(|b| b.name.as_deref() == Some("Bash")));
        let agent_call = bs
            .iter()
            .find(|b| b.name.as_deref() == Some("Agent"))
            .unwrap();
        assert_eq!(
            agent_call.id.as_deref(),
            Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243")
        );
        // the denied rm still has an (error) tool_result, so nothing is left pending
        assert!(t.pending().is_empty(), "{:?}", t.pending());
    }

    #[test]
    fn incremental_reads_and_pending_tools() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let src = std::fs::read_to_string(fixture("steer_ask_permission.jsonl")).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        let cut = lines
            .iter()
            .position(|l| l.contains("\"name\": \"AskUserQuestion\""))
            .unwrap();
        // write up to and including the AskUserQuestion tool_use, plus half of the next line
        let mut head = lines[..=cut].join("\n");
        head.push('\n');
        head.push_str(&lines[cut + 1][..10]);
        std::fs::write(&main, &head).unwrap();
        let mut t = Tailer::new(&main, true);
        assert_eq!(
            t.pending_named("AskUserQuestion").map(|p| p.name.as_str()),
            Some("AskUserQuestion")
        );
        assert!(
            t.poll().is_empty(),
            "catch-up is silent and the partial line waits"
        );
        let mut rest = lines[cut + 1..].join("\n");
        rest.push('\n');
        std::fs::write(
            &main,
            format!("{}{}", lines[..=cut].join("\n") + "\n", rest),
        )
        .unwrap();
        let evs = t.poll();
        assert!(!evs.is_empty());
        assert!(t.pending_named("AskUserQuestion").is_none());
    }

    #[test]
    fn main_blocks_reads_whole_file() {
        let bs = main_blocks(&fixture("steer_ask_permission.jsonl"));
        assert!(bs.len() > 20);
        assert_eq!(bs[0].kind, BlockKind::User);
    }
}
