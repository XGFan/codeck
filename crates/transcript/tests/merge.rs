//! Copies of one Session merge by uuid union; first occurrence wins within a file.
#[path = "../../codeck-core/tests/common/fixtures.rs"]
mod fixtures;

use std::path::{Path, PathBuf};

use fixtures::{fixture_dir, ids};
use transcript::assemble::{LoadedFile, build_skeleton, summarize};
use transcript::model::FileRole;
use transcript::parse::{parse_bytes, read_entries};

fn load(path: &Path) -> LoadedFile {
    let chunk = read_entries(path, 0).expect("read fixture");
    LoadedFile {
        path: path.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: chunk.records,
    }
}

fn from_lines(name: &str, lines: &[&str]) -> LoadedFile {
    let text = lines.join("\n") + "\n";
    LoadedFile {
        path: PathBuf::from(name),
        role: FileRole::Main,
        agent_id: None,
        records: parse_bytes(text.as_bytes(), 0).records,
    }
}

fn copy(dir: &str, sid: &str) -> LoadedFile {
    load(
        &fixture_dir("copies")
            .join("projects")
            .join(dir)
            .join(format!("{sid}.jsonl")),
    )
}

#[test]
fn union_of_prefix_and_superset_equals_superset_in_any_order() {
    let superset = || copy(ids::COPIES_DIR, ids::COPIES_SUPERSET);
    let prefix = || copy(ids::COPIES_DIR_OLD, ids::COPIES_SUPERSET);
    let alone = build_skeleton(&[superset()]);
    for files in [vec![prefix(), superset()], vec![superset(), prefix()]] {
        let s = build_skeleton(&files);
        assert_eq!(s.nodes.len(), 8);
        assert_eq!(
            s.duplicate_uuids, 0,
            "copies in other files are not duplicates"
        );
        let path: Vec<&str> = s
            .main_path
            .iter()
            .map(|&i| s.nodes[i].uuid.as_str())
            .collect();
        let alone_path: Vec<&str> = alone
            .main_path
            .iter()
            .map(|&i| alone.nodes[i].uuid.as_str())
            .collect();
        assert_eq!(path, alone_path);
        assert_eq!(path.len(), 8);
        let a = summarize(&s);
        assert_eq!(a.message_count, 8, "4 human prompts + 4 assistant messages");
        assert_eq!(a.tokens.input, 20.0);
        assert_eq!(a.tokens.output, 480.0);
        assert_eq!(a.tokens.cache_read, 80000.0);
        assert_eq!(a.tokens.cache_creation, 4000.0);
        assert_eq!(a.leaf_uuid.as_deref().map(|u| &u[..8]), Some("cb43e204"));
    }
}

#[test]
fn identical_copies_merge_without_duplicates() {
    let s = build_skeleton(&[
        copy(ids::COPIES_DIR, ids::COPIES_IDENTICAL),
        copy(ids::COPIES_DIR_OLD, ids::COPIES_IDENTICAL),
    ]);
    assert_eq!(s.nodes.len(), 4);
    assert_eq!(s.duplicate_uuids, 0);
    assert_eq!(summarize(&s).message_count, 4);
}

#[test]
fn union_keeps_entries_only_in_the_smaller_copy() {
    let big = from_lines(
        "big.jsonl",
        &[
            r#"{"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"first"}}"#,
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:02Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"one"}]}}"#,
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":"second, padded to make this file the larger copy of the two"}}"#,
        ],
    );
    let small = from_lines(
        "small.jsonl",
        &[
            r#"{"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"first"}}"#,
            r#"{"type":"user","uuid":"x9","parentUuid":"u1","timestamp":"2026-01-01T00:00:09Z","message":{"role":"user","content":"only here"}}"#,
        ],
    );
    let s = build_skeleton(&[small, big]);
    assert_eq!(s.nodes.len(), 4);
    assert!(s.by_uuid.contains_key("x9"));
    // The largest file ranks first in merged order.
    assert_eq!(s.nodes[0].uuid, "u1");
    assert_eq!(s.nodes[0].order.0, 0);
    assert_eq!(s.nodes[s.by_uuid["x9"]].order.0, 1);
    assert_eq!(s.duplicate_uuids, 0);
}

#[test]
fn first_occurrence_wins_within_a_file_and_is_counted() {
    let path = fixture_dir("basic")
        .join("projects")
        .join(ids::BASIC_DIR)
        .join(format!("{}.jsonl", ids::BASIC));
    let s = build_skeleton(&[load(&path)]);
    assert_eq!(s.nodes.len(), 66);
    assert_eq!(s.duplicate_uuids, 1);
    assert_eq!(summarize(&s).duplicate_uuids, 1);
    // The skill_listing attachment is written twice; the first copy's parent is kept.
    let dup = s
        .nodes
        .iter()
        .find(|n| n.uuid.starts_with("d00457e0"))
        .unwrap();
    let parent = &s.nodes[dup.parent.unwrap()];
    assert!(parent.uuid.starts_with("9d7d0aa7"));

    let f = from_lines(
        "dups.jsonl",
        &[
            r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"attachment","uuid":"att","parentUuid":"u1","attachment":{"type":"a"}}"#,
            r#"{"type":"attachment","uuid":"att","parentUuid":"zz","attachment":{"type":"b"}}"#,
            r#"{"type":"attachment","uuid":"att","parentUuid":"u1","attachment":{"type":"c"}}"#,
        ],
    );
    let s = build_skeleton(&[f]);
    assert_eq!(s.nodes.len(), 2);
    assert_eq!(s.duplicate_uuids, 2);
    assert_eq!(s.nodes[1].record, 1, "the first record wins");
}
