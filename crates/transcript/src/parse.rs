//! Line reader with exact byte offsets, partial-tail handling and failure counting.
//!
//! Only complete lines (terminated by `\n`) are consumed, so a line Claude Code is still writing
//! is picked up on the next read from `consumed_to`. Blank lines are skipped silently; lines that
//! are not a JSON object are counted in `failed` and skipped.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::raw::RawEntry;

#[derive(Debug)]
pub struct EntryRecord {
    /// 1-based line number counted from the chunk's `base_offset` (absolute only when parsing from 0).
    pub line_no: u32,
    /// Absolute byte offset of the line start; also the entry's file order.
    pub offset: u64,
    pub entry: RawEntry,
}

#[derive(Debug, Default)]
pub struct ParsedChunk {
    pub records: Vec<EntryRecord>,
    /// Absolute offset just past the last consumed `\n` (resume reading here).
    pub consumed_to: u64,
    pub failed: u32,
    pub first_error: Option<String>,
}

/// Reads and parses `path` from byte `from`, stopping at the last `\n`. Opens the file read-only.
/// If the file is shorter than `from`, returns an empty chunk with `consumed_to == from`.
pub fn read_entries(path: &Path, from: u64) -> io::Result<ParsedChunk> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= from {
        return Ok(ParsedChunk {
            consumed_to: from,
            ..ParsedChunk::default()
        });
    }
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    file.read_to_end(&mut buf)?;
    Ok(parse_bytes(&buf, from))
}

/// Parses the complete lines of `buf`, whose first byte sits at absolute offset `base_offset`.
pub fn parse_bytes(buf: &[u8], base_offset: u64) -> ParsedChunk {
    let mut chunk = ParsedChunk {
        consumed_to: base_offset,
        ..ParsedChunk::default()
    };
    let mut start = 0usize;
    for (idx, nl) in memchr::memchr_iter(b'\n', buf).enumerate() {
        let line_no = idx as u32 + 1;
        let offset = base_offset + start as u64;
        let line = buf[start..nl].trim_ascii();
        start = nl + 1;
        if line.is_empty() {
            continue;
        }
        // serde would also accept a JSON array as a struct; an entry must be an object.
        let parsed = if line[0] == b'{' {
            serde_json::from_slice::<RawEntry>(line).map_err(|e| e.to_string())
        } else {
            Err("不是 JSON 对象".to_owned())
        };
        match parsed {
            Ok(mut entry) => {
                entry.normalize_queued_command();
                entry.normalize_local_command();
                chunk.records.push(EntryRecord {
                    line_no,
                    offset,
                    entry,
                });
            }
            Err(e) => {
                chunk.failed += 1;
                if chunk.first_error.is_none() {
                    chunk.first_error = Some(format!("字节偏移 {offset}：{e}"));
                }
            }
        }
    }
    chunk.consumed_to = base_offset + start as u64;
    chunk
}
