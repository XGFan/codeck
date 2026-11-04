//! Diagnostics (T4.3) on fixture copies through the Engine: failed lines, unknown entry / block /
//! system-subtype names with their versions, version table and orphan Subagent Runs.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::{copy_dir_all, fixture_dir, ids};
use codeck_core::model::{DataRootSource, Diagnostics, NameCount, SessionQuery};
use codeck_core::paths::index_db_path;
use codeck_core::{Engine, EngineConfig};
use serde_json::Value;
use tempfile::TempDir;

struct Env {
    root: TempDir,
    cache: TempDir,
    engine: Engine,
}

fn scanned(scenarios: &[&str]) -> Env {
    let root = tempfile::tempdir().unwrap();
    for s in scenarios {
        copy_dir_all(&fixture_dir(s), root.path()).unwrap();
    }
    let cache = tempfile::tempdir().unwrap();
    let engine = Engine::open(EngineConfig {
        data_root: root.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();
    Env {
        root,
        cache,
        engine,
    }
}

fn transcripts(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if p.is_dir() {
            transcripts(&p, out);
        } else if name.ends_with(".jsonl") && name != "journal.jsonl" {
            out.push(p);
        }
    }
}

/// Versions of the parseable lines matching `pred` in `file`.
fn versions_where(file: &Path, pred: impl Fn(&Value) -> bool) -> (u32, Vec<String>) {
    let mut n = 0;
    let mut vs = BTreeSet::new();
    for line in fs::read_to_string(file).unwrap().lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if pred(&e) {
            n += 1;
            if let Some(v) = e["version"].as_str() {
                vs.insert(v.to_owned());
            }
        }
    }
    (n, vs.into_iter().collect())
}

fn find<'a>(list: &'a [NameCount], name: &str) -> &'a NameCount {
    list.iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("{name} missing from {list:?}"))
}

fn diagnostics(env: &Env) -> Diagnostics {
    env.engine.diagnostics().unwrap()
}

#[test]
fn reports_corrupt_line_and_unknown_names_with_versions() {
    let env = scanned(&["basic", "subagents", "workflow"]);
    let d = diagnostics(&env);
    let basic = fs::canonicalize(
        env.root
            .path()
            .join("projects")
            .join(ids::BASIC_DIR)
            .join(format!("{}.jsonl", ids::BASIC)),
    )
    .unwrap();

    let mut files = Vec::new();
    transcripts(&env.root.path().join("projects"), &mut files);
    assert_eq!(d.files_scanned as usize, files.len());
    assert_eq!((d.sessions, d.empty_sessions), (3, 0));

    // basic line 66 is truncated mid-write.
    assert_eq!(d.failed_lines, 1);
    // basic repeats one uuid inside its own file (README); the other scenarios have none.
    assert_eq!(d.duplicate_uuids, 1);
    assert_eq!(d.files_with_failures.len(), 1);
    let f = &d.files_with_failures[0];
    assert_eq!(Path::new(&f.path), basic);
    assert_eq!(f.failed_lines, 1);
    assert!(!f.first_error.is_empty());

    let (n, vs) = versions_where(&basic, |e| e["type"] == "hologram");
    let entry = find(&d.unknown_entry_types, "hologram");
    assert_eq!((entry.count, &entry.versions), (n, &vs));
    assert_eq!(d.unknown_entry_types.len(), 1);

    let (n, vs) = versions_where(&basic, |e| {
        e["message"]["content"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["type"] == "hologram_block"))
    });
    let block = find(&d.unknown_block_types, "hologram_block");
    assert_eq!((block.count, &block.versions), (n, &vs));
    assert_eq!(block.versions, vec!["2.1.207".to_owned()]);

    let (n, vs) = versions_where(&basic, |e| e["subtype"] == "future_notice");
    let sub = find(&d.unknown_system_subtypes, "future_notice");
    assert_eq!((sub.count, &sub.versions), (n, &vs));
    assert_eq!(sub.versions, vec!["2.1.207".to_owned()]);

    // The mcp__ call in basic is a known tool class; every other tool is built in.
    let (mcp, _) = versions_where(&basic, |e| {
        e["message"]["content"].as_array().is_some_and(|b| {
            b.iter().any(|b| {
                b["type"] == "tool_use"
                    && b["name"].as_str().is_some_and(|n| n.starts_with("mcp__"))
            })
        })
    });
    assert!(mcp > 0);
    assert!(d.unknown_tools.is_empty(), "{:?}", d.unknown_tools);
}

#[test]
fn version_table_and_orphans() {
    let env = scanned(&["basic", "subagents", "workflow"]);
    let d = diagnostics(&env);
    let sessions = env.engine.list_sessions(&SessionQuery::default()).unwrap();
    let basic = sessions.iter().find(|s| s.id == ids::BASIC).unwrap();

    // Newest first, numerically.
    let names: Vec<&str> = d.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(names.first(), Some(&"2.1.222"));
    assert!(names.contains(&"2.1.114"));
    // 2.1.222 is basic's newest version: it carries basic's corrupt line.
    let newest = &d.versions[0];
    assert_eq!(newest.sessions, 1);
    assert_eq!(newest.failed_lines, 1);
    assert_eq!(newest.last_seen_ms, basic.last_active_ms);
    // 2.1.207 is shared by all three sessions and holds the versioned unknown items.
    let shared = d.versions.iter().find(|v| v.version == "2.1.207").unwrap();
    assert_eq!(shared.sessions, 3);
    assert_eq!(shared.failed_lines, 0);
    assert_eq!(shared.unknown_items, 2, "hologram_block + future_notice");
    assert_eq!(
        d.versions.iter().map(|v| v.failed_lines).sum::<u32>(),
        d.failed_lines
    );

    // subagents: `a0rphan…` points at a tool_use that exists nowhere; the nested run resolves via
    // its parent, the teammate has no toolUseId, workflow agents belong to their run.
    assert_eq!(d.orphan_subagents, 1);
}

#[test]
fn names_known_since_indexing_are_dropped() {
    let env = scanned(&["basic"]);
    // An index built before these names were added to `known` still holds their counts.
    let conn = rusqlite::Connection::open(index_db_path(env.cache.path())).unwrap();
    conn.execute_batch(
        "INSERT INTO diag_counts (file_id, category, name, version, count)
         SELECT min(id), 0, 'api-request', '2.1.207', 5 FROM files;
         INSERT INTO diag_counts (file_id, category, name, version, count)
         SELECT min(id), 2, 'SendFeedback', '2.1.207', 1 FROM files;",
    )
    .unwrap();

    let d = diagnostics(&env);
    assert!(
        d.unknown_entry_types
            .iter()
            .all(|n| n.name != "api-request")
    );
    assert!(d.unknown_tools.iter().all(|n| n.name != "SendFeedback"));
    let shared = d.versions.iter().find(|v| v.version == "2.1.207").unwrap();
    assert_eq!(shared.unknown_items, 2, "hologram_block + future_notice");
}

#[test]
fn empty_index_is_all_zero() {
    let env = scanned(&[]);
    assert_eq!(diagnostics(&env), Diagnostics::default());
}
