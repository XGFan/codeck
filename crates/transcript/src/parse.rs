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
    /// The read stopped at [`ReadOptions::max_bytes`]: more complete lines may follow.
    pub capped: bool,
    /// Length of the file when it was read.
    pub file_len: u64,
}

/// How [`read_entries_with`] reads.
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    /// Read at most this many bytes from `from` (`None`: to the end). A single line longer than
    /// the cap is skipped (the bytes read are consumed; the rest of the line then fails to parse
    /// and is skipped too) instead of being re-read forever.
    pub max_bytes: Option<u64>,
    /// Rewrite `queued_command` attachments and `local_command` system entries into the user
    /// messages they stand for ([`RawEntry::normalize_queued_command`]). Off for a caller that
    /// needs a queued steer apart from a prompt.
    pub normalize: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            max_bytes: None,
            normalize: true,
        }
    }
}

/// Reads and parses `path` from byte `from`, stopping at the last `\n`. Opens the file read-only.
/// If the file is shorter than `from`, returns an empty chunk with `consumed_to == from`.
pub fn read_entries(path: &Path, from: u64) -> io::Result<ParsedChunk> {
    read_entries_with(path, from, ReadOptions::default())
}

/// [`read_entries`] with a read cap and optional normalization.
pub fn read_entries_with(path: &Path, from: u64, opts: ReadOptions) -> io::Result<ParsedChunk> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= from {
        return Ok(ParsedChunk {
            consumed_to: from,
            file_len: len,
            ..ParsedChunk::default()
        });
    }
    file.seek(SeekFrom::Start(from))?;
    let want = opts.max_bytes.map_or(len - from, |m| (len - from).min(m));
    let mut buf = Vec::with_capacity(want as usize);
    file.take(want).read_to_end(&mut buf)?;
    let mut chunk = parse_bytes_with(&buf, from, opts.normalize);
    chunk.file_len = len;
    // Bytes left unread: a read that ends exactly at the end of the file is not capped.
    chunk.capped = want < len - from;
    if chunk.capped && chunk.consumed_to == from {
        chunk.consumed_to = from + buf.len() as u64;
    }
    Ok(chunk)
}

/// Parses the complete lines of `buf`, whose first byte sits at absolute offset `base_offset`.
pub fn parse_bytes(buf: &[u8], base_offset: u64) -> ParsedChunk {
    parse_bytes_with(buf, base_offset, true)
}

fn parse_bytes_with(buf: &[u8], base_offset: u64, normalize: bool) -> ParsedChunk {
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
                if normalize {
                    entry.normalize_queued_command();
                    entry.normalize_local_command();
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn read_entries_with_cap_and_normalize() {
        let d = tempfile::tempdir().unwrap();
        let l1 = "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"a\"}}\n";
        let l2 = "{\"type\":\"user\",\"uuid\":\"u2\",\"message\":{\"role\":\"user\",\"content\":\"b\"}}\n";
        let l3 = "{\"type\":\"user\",\"uuid\":\"u3\",\"message\":{\"role\":\"user\",\"content\":\"c\"}}\n";
        let body = format!("{l1}{l2}{l3}");
        let p = write_temp(d.path(), "capped.jsonl", &body);
        let (l1_len, len) = (l1.len() as u64, body.len() as u64);

        // The cap ends inside the second line: one complete record, the rest resumed later.
        let c = read_entries_with(
            &p,
            0,
            ReadOptions {
                max_bytes: Some(l1_len + 5),
                normalize: true,
            },
        )
        .unwrap();
        assert_eq!(c.records.len(), 1);
        assert_eq!(c.records[0].entry.entry_type(), "user");
        assert_eq!(c.records[0].offset, 0);
        assert!(c.capped);
        assert_eq!(c.consumed_to, l1_len);
        assert_eq!(c.file_len, len);

        // A cap shorter than the first line consumes what it read instead of re-reading it.
        let c = read_entries_with(
            &p,
            0,
            ReadOptions {
                max_bytes: Some(3),
                normalize: true,
            },
        )
        .unwrap();
        assert!(c.records.is_empty());
        assert!(c.capped);
        assert_eq!(c.consumed_to, 3);
        assert_eq!(c.file_len, len);

        // A cap that reaches exactly the end of the file is not a capped read: a partial last line
        // stays unread (it is re-read once complete) instead of being skipped.
        let partial = write_temp(d.path(), "partial.jsonl", &format!("{l1}{{\"type\":\"us"));
        let plen = std::fs::metadata(&partial).unwrap().len();
        let opts = ReadOptions {
            max_bytes: Some(plen),
            normalize: true,
        };
        let c = read_entries_with(&partial, 0, opts).unwrap();
        assert_eq!(c.records.len(), 1);
        assert!(!c.capped);
        assert_eq!(c.consumed_to, l1_len);
        let c = read_entries_with(
            &partial,
            l1_len,
            ReadOptions {
                max_bytes: Some(plen - l1_len),
                normalize: true,
            },
        )
        .unwrap();
        assert!(c.records.is_empty() && !c.capped);
        assert_eq!(c.consumed_to, l1_len);

        // Starting past the end: nothing to read, resume where we asked to.
        let c = read_entries(&p, len + 10).unwrap();
        assert!(c.records.is_empty());
        assert_eq!(c.consumed_to, len + 10);
        assert_eq!(c.file_len, len);

        // A `queued_command` attachment is a user message once normalized.
        let q = write_temp(
            d.path(),
            "queued.jsonl",
            "{\"type\":\"attachment\",\"uuid\":\"q1\",\"attachment\":{\"type\":\"queued_command\",\"prompt\":\"steer me\"}}\n",
        );
        let on = read_entries_with(
            &q,
            0,
            ReadOptions {
                max_bytes: None,
                normalize: true,
            },
        )
        .unwrap();
        assert_eq!(on.records.len(), 1);
        assert_eq!(on.records[0].entry.entry_type(), "user");
        assert_eq!(on.records[0].entry.content_text(), Some("steer me"));
        let off = read_entries_with(
            &q,
            0,
            ReadOptions {
                max_bytes: None,
                normalize: false,
            },
        )
        .unwrap();
        assert_eq!(off.records.len(), 1);
        assert_eq!(off.records[0].entry.entry_type(), "attachment");

        // Lenient scalars: fields of another JSON type become `None`, the line still parses.
        let f = write_temp(
            d.path(),
            "fields.jsonl",
            "{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}\n\
             {\"type\":\"system\",\"subtype\":\"turn_duration\",\"durationMs\":4200}\n\
             {\"type\":\"assistant\",\"isApiErrorMessage\":true,\"error\":\"rate_limit\"}\n\
             {\"type\":\"system\",\"subtype\":\"turn_duration\",\"durationMs\":\"oops\"}\n",
        );
        let c = read_entries(&f, 0).unwrap();
        assert_eq!(c.records.len(), 4);
        assert_eq!(c.failed, 0);
        assert_eq!(c.records[0].entry.operation.as_deref(), Some("enqueue"));
        assert_eq!(c.records[1].entry.duration_ms, Some(4200.0));
        assert_eq!(c.records[2].entry.error.as_deref(), Some("rate_limit"));
        assert_eq!(c.records[3].entry.duration_ms, None);
        assert_eq!(c.records[3].entry.subtype.as_deref(), Some("turn_duration"));
    }
}
