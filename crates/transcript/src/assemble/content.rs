//! Full tool input/output (including persisted `tool-results/<basename>`) and image data.

use std::borrow::Cow;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::value::RawValue;

use super::nodes::{persisted_ref, result_content};
use super::{AssembledSession, LoadedFile, SessionSkeleton, skeleton};
use crate::error::{TranscriptError, TranscriptResult};
use crate::model::{
    DetailSource, ImageData, ImageRequest, ToolDetail, ToolDetailRequest, ToolPart, TranscriptScope,
};
use crate::raw::{RawBlock, RawEntry};

/// `ToolDetail` cap.
pub const DETAIL_BYTES: usize = 8 * 1024 * 1024;

/// Basename of a persisted-output path: only the last component counts; empty, `.`, `..`
/// or anything containing `..` is rejected.
pub fn persisted_basename(path: &str) -> Option<String> {
    let base = path.trim().rsplit(['/', '\\']).next()?;
    (!base.is_empty() && base != "." && !base.contains("..")).then(|| base.to_owned())
}

/// `<session_dir>/tool-results/<name>`; names containing `/`, `\` or `..` are rejected.
pub fn persisted_path(session_dir: &Path, name: &str) -> Option<PathBuf> {
    let bad = name.is_empty() || name == "." || name.contains(['/', '\\']) || name.contains("..");
    (!bad).then(|| session_dir.join("tool-results").join(name))
}

/// The skeleton and files of the requested scope (agent files are assembled on demand).
fn scope_view<'a>(
    s: &'a AssembledSession,
    scope: &TranscriptScope,
    agent_file: Option<&'a LoadedFile>,
) -> TranscriptResult<(Cow<'a, SessionSkeleton>, &'a [LoadedFile])> {
    match scope {
        TranscriptScope::Main => Ok((Cow::Borrowed(&s.skeleton), &s.main_files)),
        TranscriptScope::Subagent { agent_id } => {
            let f = agent_file.ok_or_else(|| {
                TranscriptError::NotFound(format!("Subagent {agent_id} 的记录文件"))
            })?;
            let files = std::slice::from_ref(f);
            Ok((Cow::Owned(skeleton::build(files)), files))
        }
    }
}

fn find_block<'a>(e: &'a RawEntry, kind: &str, id: &str) -> Option<&'a RawBlock> {
    e.blocks().iter().find(|b| {
        b.block_type() == kind
            && match kind {
                "tool_use" => b.id.as_deref() == Some(id),
                _ => b.tool_use_id.as_deref() == Some(id),
            }
    })
}

fn capped(text: String) -> (String, bool) {
    if text.len() <= DETAIL_BYTES {
        return (text, false);
    }
    let mut cut = DETAIL_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_owned(), true)
}

/// Reads at most [`DETAIL_BYTES`] of a persisted output file (read-only); returns
/// `(text, truncated, size)`. A symlink is refused (`O_NOFOLLOW`, as ccrpc does), so a link
/// planted in `tool-results/` cannot point the read elsewhere; anything but a regular file (a
/// FIFO would block the read forever) counts as missing.
pub(crate) fn read_capped(path: &Path) -> std::io::Result<(String, bool, u64)> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let md = f.metadata()?;
    if !md.is_file() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let size = md.len();
    let mut buf = Vec::with_capacity(size.min(DETAIL_BYTES as u64) as usize);
    f.take(DETAIL_BYTES as u64).read_to_end(&mut buf)?;
    let text = match String::from_utf8(buf) {
        Ok(t) => t,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            if size > DETAIL_BYTES as u64 && bytes.len() - valid < 4 {
                bytes.truncate(valid); // cut inside a multi-byte char at the cap
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    Ok((text, size > DETAIL_BYTES as u64, size))
}

/// `ToolPart::Output` of a persisted result reads `<session_dir>/tool-results/<basename>` and
/// rejects names containing `/` or `..`.
pub fn tool_detail(
    s: &AssembledSession,
    r: &ToolDetailRequest,
    agent_file: Option<&LoadedFile>,
) -> TranscriptResult<ToolDetail> {
    let (sk, files) = scope_view(s, &r.scope, agent_file)?;
    let id = r.tool_use_id.as_str();
    match r.part {
        ToolPart::Input => {
            if !sk.tool_use_at.contains_key(id) {
                return Err(TranscriptError::NotFound("该工具调用".to_owned()));
            }
            let raw = sk
                .tool_use_block(files, id)
                .and_then(|(_, b)| b.input.as_deref())
                .map(|v| v.get().to_owned())
                .unwrap_or_else(|| "{}".to_owned());
            let total = raw.len() as f64;
            let (text, truncated) = capped(raw);
            Ok(ToolDetail {
                text,
                truncated,
                total_bytes: total,
                source: DetailSource::Inline,
            })
        }
        ToolPart::Output => {
            // Always the first result: the request carries no branch choices, so the display
            // path (which the card and ⌘F prefer) is unknown here.
            let Some((_, e, block)) = sk.tool_result_block(files, id, None) else {
                if !sk.tool_use_at.contains_key(id) {
                    return Err(TranscriptError::NotFound("该工具调用".to_owned()));
                }
                return Ok(ToolDetail {
                    text: String::new(),
                    truncated: false,
                    total_bytes: 0.0,
                    source: DetailSource::Missing,
                });
            };
            let (inline, _) = result_content(block.content.as_deref());
            if let Some(p) = persisted_ref(e, &inline)
                && let Some(path) = s.persisted_file(&p.file_name)
            {
                if let Ok((text, truncated, size)) = read_capped(&path) {
                    return Ok(ToolDetail {
                        text,
                        truncated,
                        total_bytes: size as f64,
                        source: DetailSource::PersistedFile,
                    });
                }
                let total = inline.len() as f64;
                let (text, truncated) = capped(inline);
                return Ok(ToolDetail {
                    text,
                    truncated,
                    total_bytes: total,
                    source: DetailSource::Missing,
                });
            }
            let total = inline.len() as f64;
            let (text, truncated) = capped(inline);
            Ok(ToolDetail {
                text,
                truncated,
                total_bytes: total,
                source: DetailSource::Inline,
            })
        }
    }
}

#[derive(Deserialize)]
struct Item<'a> {
    #[serde(rename = "type", default)]
    item_type: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    source: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Source {
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

fn image_data(source: Option<&RawValue>) -> Option<ImageData> {
    let src: Source = serde_json::from_str(source?.get()).ok()?;
    // The type ends up in a `data:` URL; only pass through raster formats an <img> renders.
    const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
    let media_type = src
        .media_type
        .filter(|m| IMAGE_TYPES.contains(&m.as_str()))
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    Some(ImageData {
        media_type,
        data_base64: src.data?,
    })
}

pub fn image(
    s: &AssembledSession,
    r: &ImageRequest,
    agent_file: Option<&LoadedFile>,
) -> TranscriptResult<ImageData> {
    let (sk, files) = scope_view(s, &r.scope, agent_file)?;
    let ordinal = r.image.ordinal as usize;
    let found = match &r.image.tool_use_id {
        Some(id) => sk.results_for.get(id).and_then(|rs| {
            rs.iter().find_map(|&ri| {
                let block = find_block(sk.entry(files, ri), "tool_result", id)?;
                let items: Vec<Item<'_>> =
                    serde_json::from_str(block.content.as_deref()?.get()).ok()?;
                items
                    .into_iter()
                    .filter(|it| it.item_type.as_deref() == Some("image"))
                    .nth(ordinal)
                    .and_then(|it| image_data(it.source))
            })
        }),
        None => sk.by_uuid.get(&r.image.node_id).and_then(|&i| {
            sk.entry(files, i)
                .blocks()
                .iter()
                .filter(|b| b.block_type() == "image")
                .nth(ordinal)
                .and_then(|b| image_data(b.source.as_deref()))
        }),
    };
    found.ok_or_else(|| TranscriptError::NotFound("该图片".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_names_are_basename_only() {
        assert_eq!(
            persisted_basename("/Users/x/.claude/tool-results/b8e6n2j5e.txt").as_deref(),
            Some("b8e6n2j5e.txt")
        );
        assert_eq!(
            persisted_basename("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(persisted_basename("/a/b/.."), None);
        assert_eq!(persisted_basename("/a/b/"), None);
        let dir = Path::new("/s");
        assert_eq!(
            persisted_path(dir, "x.txt"),
            Some(PathBuf::from("/s/tool-results/x.txt"))
        );
        assert_eq!(persisted_path(dir, "../x.txt"), None);
        assert_eq!(persisted_path(dir, "a/b.txt"), None);
        assert_eq!(persisted_path(dir, ".."), None);
    }
}
