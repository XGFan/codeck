//! Writing computed sessions, edited Projects and billing inside the sync transactions.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{Connection, OptionalExtension};

use super::compute::SessionWork;
use crate::engine::ChangeSet;
use crate::error::CoreResult;
use crate::index::writer::{self, FileWrite, ProjectRow, SessionRow};
use crate::project;

// -------------------------------------------------------------------- write

/// `is_empty` of an existing session row.
pub(super) fn session_exists(conn: &Connection, sid: &str) -> CoreResult<Option<bool>> {
    Ok(conn
        .prepare_cached("SELECT is_empty FROM sessions WHERE id=?1")?
        .query_row([sid], |r| r.get::<_, bool>(0))
        .optional()?)
}

/// Above this many changed sessions (a full scan), billing is redone for every session instead of
/// looking up the neighbors of each.
pub(super) const REBILL_ALL_ABOVE: usize = 200;

/// Re-derives which sessions are superseded among the changed and removed ones and those continued
/// in them (all of them when `all` or after a full scan); a session whose flag changed is reported
/// as changed (and billed again by [`rebill_changed`]), and the Project counts as changed. Returns
/// those sessions.
pub(super) fn update_superseded(
    conn: &Connection,
    cs: &mut ChangeSet,
    all: bool,
) -> CoreResult<Vec<String>> {
    let flipped = if all || cs.changed.len() > REBILL_ALL_ABOVE {
        writer::update_superseded(conn, None)?
    } else {
        let scope: HashSet<String> = cs.changed.iter().chain(&cs.removed).cloned().collect();
        writer::update_superseded(conn, Some(&scope))?
    };
    if !flipped.is_empty() {
        cs.projects_changed = true;
        cs.changed.extend(flipped.iter().cloned());
    }
    Ok(flipped)
}

/// Re-bills the changed sessions and every session sharing a message with them; those whose
/// totals may have moved are reported as changed too.
pub(super) fn rebill_changed(
    conn: &Connection,
    cs: &mut ChangeSet,
    mut neighbors: HashSet<String>,
) -> CoreResult<()> {
    if cs.changed.is_empty() && neighbors.is_empty() {
        return Ok(());
    }
    if cs.changed.len() > REBILL_ALL_ABOVE {
        return writer::rebill(conn, None);
    }
    for sid in &cs.changed {
        neighbors.extend(writer::copy_neighbors(conn, sid)?);
    }
    let removed: HashSet<&String> = cs.removed.iter().collect();
    neighbors.retain(|s| !removed.contains(s));
    let mut scope: HashSet<String> = cs.changed.iter().cloned().collect();
    for n in neighbors {
        if scope.insert(n.clone()) {
            cs.changed.push(n);
        }
    }
    writer::rebill(conn, Some(&scope))
}

pub(super) fn write_work(
    conn: &Connection,
    w: &SessionWork,
    cs: &mut ChangeSet,
    neighbors: &mut HashSet<String>,
    resolver: &mut project::Resolver,
) -> CoreResult<()> {
    for (id, size, mtime) in &w.stat_only {
        writer::write_file_stat(conn, *id, *size, *mtime)?;
    }
    if !w.recompute {
        return Ok(());
    }
    let before = session_exists(conn, &w.sid)?;
    if w.reparse {
        neighbors.extend(writer::copy_neighbors(conn, &w.sid)?);
        writer::delete_session(conn, &w.sid)?;
    }
    for f in &w.files {
        let path = f.file.path.to_string_lossy();
        let id = writer::write_file(
            conn,
            &FileWrite {
                path: &path,
                dev: f.file.dev,
                ino: f.file.ino,
                role: f.file.role,
                session_id: &w.sid,
                agent_id: f.file.agent_id.as_deref(),
                size: f.file.size,
                mtime_ns: f.file.mtime_ns,
                head: &f.head,
                parsed_offset: f.parsed_offset,
                line_count: f.line_count,
                failed_lines: f.failed_lines,
                first_error: f.first_error.as_deref(),
            },
        )?;
        // Phase 2: text docs for `[text_offset, parsed_offset)` are extracted by
        // `index::text::index_pending`; `text_offset` is deliberately left untouched here.
        writer::write_rows(
            conn,
            id,
            &w.sid,
            f.file.agent_id.as_deref().unwrap_or(""),
            &f.rows,
        )?;
    }
    let Some(s) = &w.summary else {
        if before.is_some() {
            conn.execute("DELETE FROM sessions WHERE id=?1", [&w.sid])?;
            writer::write_session_projects(conn, &w.sid, &[])?;
            cs.removed.push(w.sid.clone());
            cs.projects_changed = true;
        }
        return Ok(());
    };
    writer::write_subagents(conn, &w.sid, &w.agents)?;
    let a = &s.agg;
    // Placeholders: `rebill_changed` re-sums the token columns from billed messages.
    let main_tok = a.tokens.clone();
    let row = SessionRow {
        id: w.sid.clone(),
        project_id: s.project.id.clone(),
        cwd: a.cwd.clone(),
        title: a.title.clone(),
        title_source: a.title_source,
        first_prompt: a.first_prompt.clone(),
        created_ms: a.created_ms,
        last_active_ms: if a.last_active_ms > 0.0 {
            a.last_active_ms
        } else {
            s.mtime_ms
        },
        duration_ms: a.duration_ms,
        message_count: a.message_count,
        tool_call_count: a.tool_call_count,
        subagent_count: w.subagent_count,
        tokens: main_tok.clone(),
        main_tokens: main_tok,
        git_branch: a.git_branch.clone(),
        primary_model: a.primary_model.clone(),
        models: a.models.clone(),
        versions: a.versions.clone(),
        leaf_uuid: a.leaf_uuid.clone(),
        root_uuid: a.root_uuid.clone(),
        fork_origin_id: a.fork_origin_id.clone(),
        fork_point_uuid: a.fork_point_uuid.clone(),
        continued_in_id: a.continued_in_id.clone(),
        is_empty: a.is_empty,
        dup_uuids: a.duplicate_uuids,
    };
    writer::write_session(conn, &row)?;
    if writer::write_project(conn, &s.project)? || before != Some(a.is_empty) {
        cs.projects_changed = true;
    }
    if write_edited_projects(conn, &w.sid, &s.project, resolver)? {
        cs.projects_changed = true;
    }
    if let Some(ms) = &w.main_set
        && writer::has_text_rows(conn, &w.sid)?
    {
        writer::write_on_main_line(conn, &w.sid, ms)?;
    }
    cs.changed.push(w.sid.clone());
    Ok(())
}

/// Re-derives the Projects a session edited from every edit path recorded for it (main file and
/// subagents); returns true when the set or any Project row changed.
pub(super) fn write_edited_projects(
    conn: &Connection,
    sid: &str,
    start: &ProjectRow,
    resolver: &mut project::Resolver,
) -> CoreResult<bool> {
    let mut rows: BTreeMap<String, ProjectRow> = BTreeMap::new();
    for path in writer::edited_paths(conn, sid)? {
        if let Some(p) = resolver.edit_project(&path, start) {
            rows.entry(p.id.clone()).or_insert(p);
        }
    }
    let mut changed = false;
    for p in rows.values() {
        changed |= writer::write_project(conn, p)?;
    }
    let ids: Vec<String> = rows.into_keys().collect();
    changed |= writer::write_session_projects(conn, sid, &ids)?;
    Ok(changed)
}
