//! The content units a transcript message is reported as, and the tool calls still waiting.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use transcript::assemble::{content, nodes};
use transcript::raw::RawBlock;

/// Cap on text carried by one block event (tool outputs can be huge).
pub const MAX_BLOCK_TEXT: usize = 100 * 1024;

/// A tool output Claude Code saved to `tool-results/` (the transcript keeps a preview).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Persisted {
    pub path: String,
    pub size: u64,
}

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
    /// tool_result whose full output is in a file (`content` is Claude Code's preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted: Option<Persisted>,
    /// User input delivered mid-turn (typed while busy).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// A prompt's images (media types, in order); their data comes from `get_image`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Set when the block comes from a Subagent Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// Set when the Subagent Run belongs to a Workflow Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Block {
    pub(super) fn new(kind: BlockKind, uuid: &str, index: usize) -> Block {
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
            persisted: None,
            queued: false,
            images: Vec::new(),
            agent_id: None,
            parent_tool_use_id: None,
            run_id: None,
        }
    }
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

/// `s` cut to [`MAX_BLOCK_TEXT`].
pub(super) fn cap(mut s: String) -> String {
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

/// The media type of an image block (`image/png` when not recorded).
pub(super) fn image_type(b: &RawBlock) -> String {
    b.source
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s.get()).ok())
        .and_then(|v| v["media_type"].as_str().map(str::to_string))
        .unwrap_or_else(|| "image/png".into())
}

/// The file a `<persisted-output>` preview points to, only when the preview names this Session's
/// own `tool-results/` (a tool's own output could imitate the preview).
pub(super) fn persisted_path(text: &str, session_dir: &Path) -> Option<PathBuf> {
    let claimed = Path::new(nodes::persisted_output_path(text)?);
    let name = content::persisted_basename(claimed.to_str()?)?;
    let p = content::persisted_path(session_dir, &name)?;
    (claimed.parent() == p.parent()).then_some(p)
}
