//! Query parser: whitespace AND, `"phrase"`, `-exclude`.
//!
//! A token starting with `"` runs to the next `"` (or the end) and keeps its inner spaces; a
//! leading `-` turns a word or phrase into an exclusion. Quotes elsewhere in a word are literal.

use crate::error::{CoreError, CoreResult};

/// Literal substring terms (phrases keep their inner spaces).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// An all-exclusion query is `InvalidQuery`.
pub fn parse_query(q: &str) -> CoreResult<ParsedQuery> {
    let mut out = ParsedQuery::default();
    let mut rest = q;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let negate = rest
            .strip_prefix('-')
            .and_then(|r| r.chars().next())
            .is_some_and(|c| !c.is_whitespace());
        if negate {
            rest = &rest[1..];
        }
        let term;
        if let Some(body) = rest.strip_prefix('"') {
            let end = body.find('"').unwrap_or(body.len());
            term = &body[..end];
            rest = body.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            term = &rest[..end];
            rest = &rest[end..];
        }
        if term.is_empty() {
            continue;
        }
        let list = if negate {
            &mut out.exclude
        } else {
            &mut out.include
        };
        list.push(term.to_owned());
    }
    if out.include.is_empty() {
        let msg = if out.exclude.is_empty() {
            "搜索词为空"
        } else {
            "至少需要一个包含词"
        };
        return Err(CoreError::InvalidQuery(msg.to_owned()));
    }
    Ok(out)
}

/// An FTS5 phrase for a literal term: wrapped in `"` with inner quotes doubled.
pub fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Terms of at least 3 characters can use the trigram index; shorter ones fall back to `LIKE`.
pub fn is_fts_term(term: &str) -> bool {
    term.chars().nth(2).is_some()
}
