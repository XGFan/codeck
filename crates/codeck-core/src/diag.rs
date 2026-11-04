//! Format-compatibility diagnostics from `files`, `diag_counts` and version aggregation.
//!
//! - Unknown names come from `diag_counts` (`mcp__*` tools are a known class and never counted).
//!   Names recorded before they were added to [`known`] are dropped on read, so the report is
//!   current without a rebuild.
//! - A corrupt line carries no version, so a file's failed lines are attributed to the newest
//!   Claude Code version seen in its session.
//! - `orphan_subagents`: non-workflow Subagent Runs whose `meta.toolUseId` matches no main-file
//!   tool call and whose parent agent is unknown. Runs without a `toolUseId` (linked by
//!   `toolUseResult.agentId` / name at assembly time) are not counted.
//! - `duplicate_uuids`: sum of the per-session in-file duplicate counts recorded by assembly.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;

use crate::error::CoreResult;
use crate::index::schema::{DIAG_BLOCK, DIAG_ENTRY, DIAG_SYSTEM_SUBTYPE, DIAG_TOOL};
use crate::known;
use crate::model::{Diagnostics, FileFailure, NameCount, VersionStat};

fn is_known(cat: i64, name: &str) -> bool {
    match cat {
        DIAG_ENTRY => known::is_known_entry_type(name),
        DIAG_BLOCK => known::is_known_block_type(name),
        DIAG_SYSTEM_SUBTYPE => known::is_known_system_subtype(name),
        DIAG_TOOL => known::is_known_tool(name),
        _ => false,
    }
}

pub fn compute(conn: &Connection) -> CoreResult<Diagnostics> {
    let mut d = Diagnostics::default();
    let (files, failed): (u32, i64) = conn.query_row(
        "SELECT count(*), coalesce(sum(failed_lines), 0) FROM files",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    d.files_scanned = files;
    d.failed_lines = failed as u32;
    (d.sessions, d.empty_sessions) = conn.query_row(
        "SELECT count(*), coalesce(sum(is_empty), 0) FROM sessions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let mut st = conn.prepare(
        "SELECT path, failed_lines, coalesce(first_error, '') FROM files WHERE failed_lines > 0
         ORDER BY failed_lines DESC, path",
    )?;
    d.files_with_failures = st
        .query_map([], |r| {
            Ok(FileFailure {
                path: r.get(0)?,
                failed_lines: r.get(1)?,
                first_error: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Versions per session: sessions, last seen and failed lines (newest version only).
    let mut versions: HashMap<String, VersionStat> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT s.versions_json, s.last_active_ms,
            (SELECT coalesce(sum(f.failed_lines), 0) FROM files f WHERE f.session_id = s.id)
         FROM sessions s",
    )?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let list: Vec<String> = serde_json::from_str(&r.get::<_, String>(0)?).unwrap_or_default();
        let last_active = r.get::<_, i64>(1)? as f64;
        let failed: u32 = r.get(2)?;
        let newest = list.iter().max_by(|a, b| cmp_version(a, b)).cloned();
        for v in list {
            let e = versions
                .entry(v.clone())
                .or_insert_with(|| empty_version(v));
            e.sessions += 1;
            e.last_seen_ms = e.last_seen_ms.max(last_active);
        }
        if let Some(v) = newest {
            versions.get_mut(&v).expect("listed").failed_lines += failed;
        }
    }

    // Unknown names, per category, with the versions they were seen in.
    let mut names: HashMap<(i64, String), (u32, BTreeSet<String>)> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT category, name, version, sum(count) FROM diag_counts GROUP BY category, name, version",
    )?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let (cat, name): (i64, String) = (r.get(0)?, r.get(1)?);
        if is_known(cat, &name) {
            continue;
        }
        let version: String = r.get(2)?;
        let count: u32 = r.get(3)?;
        let e = names.entry((cat, name)).or_default();
        e.0 += count;
        if !version.is_empty() {
            versions
                .entry(version.clone())
                .or_insert_with(|| empty_version(version.clone()))
                .unknown_items += count;
            e.1.insert(version);
        }
    }
    let mut by_cat: HashMap<i64, Vec<NameCount>> = HashMap::new();
    for ((cat, name), (count, vs)) in names {
        let mut versions: Vec<String> = vs.into_iter().collect();
        versions.sort_by(|a, b| cmp_version(b, a));
        by_cat.entry(cat).or_default().push(NameCount {
            name,
            count,
            versions,
        });
    }
    let mut take = |cat: i64| {
        let mut v = by_cat.remove(&cat).unwrap_or_default();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        v
    };
    d.unknown_entry_types = take(DIAG_ENTRY);
    d.unknown_block_types = take(DIAG_BLOCK);
    d.unknown_system_subtypes = take(DIAG_SYSTEM_SUBTYPE);
    d.unknown_tools = take(DIAG_TOOL);

    d.versions = versions.into_values().collect();
    d.versions
        .sort_by(|a, b| cmp_version(&b.version, &a.version));

    d.duplicate_uuids = conn.query_row(
        "SELECT coalesce(sum(dup_uuids), 0) FROM sessions",
        [],
        |r| r.get(0),
    )?;

    d.orphan_subagents = conn.query_row(
        "SELECT count(*) FROM subagents a
         WHERE a.workflow_run_id IS NULL AND a.tool_use_id IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM tool_calls t WHERE t.session_id = a.session_id
                AND t.agent_id = '' AND t.tool_use_id = a.tool_use_id)
            AND NOT EXISTS (SELECT 1 FROM subagents p WHERE p.session_id = a.session_id
                AND p.agent_id = a.parent_agent_id)",
        [],
        |r| r.get(0),
    )?;
    Ok(d)
}

fn empty_version(version: String) -> VersionStat {
    VersionStat {
        version,
        sessions: 0,
        failed_lines: 0,
        unknown_items: 0,
        last_seen_ms: 0.0,
    }
}

/// Compares dotted versions numerically (`2.1.10` > `2.1.9`), falling back to text.
fn cmp_version(a: &str, b: &str) -> Ordering {
    let parts = |s: &str| -> Vec<u64> { s.split('.').map_while(|p| p.parse().ok()).collect() };
    parts(a).cmp(&parts(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(cmp_version("2.1.10", "2.1.9"), Ordering::Greater);
        assert_eq!(cmp_version("2.1.9", "2.1.9"), Ordering::Equal);
        assert_eq!(cmp_version("1.0.100", "2.0.0"), Ordering::Less);
    }
}
