//! Reading transcript files: complete lines only, read-only, capped.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;
use transcript::layout;
use transcript::parse::{ReadOptions, read_entries_with};
use transcript::raw::RawEntry;

use super::block::{MAX_BLOCK_TEXT, cap};

/// Cap on one read of a transcript file.
const MAX_READ: u64 = 8 * 1024 * 1024;

/// One complete transcript line (queued commands not rewritten: a steer stays apart).
pub(super) struct Line {
    pub(super) entry: RawEntry,
    /// Byte offset of the line in its file.
    pub(super) offset: u64,
}

pub(super) struct Chunk {
    pub(super) lines: Vec<Line>,
    pub(super) consumed_to: u64,
    /// The read stopped at the size cap: more complete lines may follow.
    pub(super) capped: bool,
}

/// Reads the complete lines of `path` from byte `from` (read-only; a partial last line is left
/// for the next read; lines that are not JSON objects are skipped), at most [`MAX_READ`] bytes,
/// without rewriting `queued_command` attachments (steer needs them apart from prompts).
pub(super) fn read_lines(path: &Path, from: u64) -> std::io::Result<Chunk> {
    let opts = ReadOptions {
        max_bytes: Some(MAX_READ),
        normalize: false,
    };
    let c = read_entries_with(path, from, opts)?;
    Ok(Chunk {
        lines: c
            .records
            .into_iter()
            .map(|r| Line {
                entry: r.entry,
                offset: r.offset,
            })
            .collect(),
        // A file that shrank: continue from its new end.
        consumed_to: c.consumed_to.min(c.file_len),
        capped: c.capped,
    })
}

/// The uuids of every entry in the transcript `path` (the main file only).
pub fn entry_uuids(path: &Path) -> HashSet<String> {
    let mut uuids = HashSet::new();
    let mut from = 0;
    while let Ok(c) = read_lines(path, from) {
        uuids.extend(c.lines.into_iter().filter_map(|l| l.entry.uuid));
        if !c.capped || c.consumed_to <= from {
            break;
        }
        from = c.consumed_to;
    }
    uuids
}

/// A regular text file read with the block cap (read-only; no symlinks, nothing that blocks).
pub(super) fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let mut buf = Vec::new();
    f.take(MAX_BLOCK_TEXT as u64 + 4)
        .read_to_end(&mut buf)
        .ok()?;
    Some(cap(String::from_utf8_lossy(&buf).into_owned()))
}

/// The tool call that started the Subagent Run `agent_id`, from its meta file in `dir`.
pub(super) fn read_meta_tool_use_id(dir: &Path, agent_id: &str) -> Option<String> {
    let bytes = std::fs::read(layout::agent_meta_path(dir, agent_id)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("toolUseId")
        .and_then(Value::as_str)
        .map(str::to_string)
}
