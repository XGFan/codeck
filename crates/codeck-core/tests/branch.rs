//! Tree (§3.4 steps 2–4 + A1/A1b) and Branch (step 7 + A14d) semantics.
mod common;

use std::path::{Path, PathBuf};

use common::{fixture_dir, ids};
use codeck_core::assemble::{
    AssembledSession, LoadedFile, SessionSidecar, SessionSkeleton, assemble, build_skeleton,
    transcript, tree,
};
use codeck_core::model::{
    AssistantBlock, BranchChoice, FileRole, NodeBody, Transcript, TranscriptRequest,
    TranscriptScope,
};
use codeck_core::parse::{parse_bytes, read_entries};

fn load(path: &Path) -> LoadedFile {
    LoadedFile {
        path: path.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: read_entries(path, 0).expect("read fixture").records,
    }
}

fn fixture(scenario: &str, dir: &str, sid: &str) -> LoadedFile {
    load(
        &fixture_dir(scenario)
            .join("projects")
            .join(dir)
            .join(format!("{sid}.jsonl")),
    )
}

fn from_lines(lines: &[&str]) -> LoadedFile {
    let text = lines.join("\n") + "\n";
    LoadedFile {
        path: PathBuf::from("inline.jsonl"),
        role: FileRole::Main,
        agent_id: None,
        records: parse_bytes(text.as_bytes(), 0).records,
    }
}

fn session(file: LoadedFile) -> AssembledSession {
    assemble(
        "s",
        Path::new("/nonexistent"),
        vec![file],
        SessionSidecar::default(),
    )
}

fn main_transcript(s: &AssembledSession, choices: Vec<BranchChoice>) -> Transcript {
    transcript(
        s,
        &TranscriptRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Main,
            branch_choices: choices,
            include_hidden: false,
        },
        None,
    )
    .unwrap()
}

fn uuid(s: &SessionSkeleton, prefix: &str) -> String {
    s.nodes
        .iter()
        .find(|n| n.uuid.starts_with(prefix))
        .unwrap_or_else(|| panic!("no node {prefix}"))
        .uuid
        .clone()
}

fn path_prefixes(s: &SessionSkeleton, path: &[usize]) -> Vec<String> {
    path.iter()
        .map(|&i| s.nodes[i].uuid.chars().take(8).collect())
        .collect()
}

fn prompt_texts(t: &Transcript) -> Vec<String> {
    t.nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::UserPrompt { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn assistant_texts(t: &Transcript) -> Vec<String> {
    t.nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::Assistant { blocks, .. } => Some(
                blocks
                    .iter()
                    .filter_map(|b| match b {
                        AssistantBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect()
}

// ---- tree ----

#[test]
fn compact_boundary_relinks_through_logical_parent_without_branch() {
    let s = session(fixture("compact", ids::COMPACT_DIR, ids::COMPACT_SIBLING));
    let sk = &s.skeleton;
    let path = path_prefixes(sk, &sk.main_path);
    assert_eq!(
        path,
        [
            "aa28f777", "9ae25975", "3fe0a4bc", "c6ce36d5", "74ef7bec", "811888d2", "b8928552",
            "2bf70509", "e910f474", "97d6b0bf"
        ]
    );
    // The `/compact` prompt is an off-path sibling of the boundary.
    assert!(!path.contains(&"affd6657".to_owned()));
    let t = main_transcript(&s, vec![]);
    assert!(t.branch_points.is_empty());
    assert_eq!(
        t.nodes
            .iter()
            .filter(|n| matches!(n.body, NodeBody::CompactBoundary { .. }))
            .count(),
        1
    );
    assert!(
        t.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::CompactSummary { .. }))
    );
    assert_eq!(
        prompt_texts(&t),
        [
            "Refactor the config loader",
            "Also support TOML",
            "Add tests for the TOML path"
        ]
    );
}

#[test]
fn missing_logical_parent_falls_back_to_preceding_entry() {
    let s = session(fixture("compact", ids::COMPACT_DIR, ids::COMPACT_MISSING));
    let sk = &s.skeleton;
    let boundary = &sk.nodes[sk.by_uuid[&uuid(sk, "2c6261a0")]];
    assert!(
        sk.nodes[boundary.parent.unwrap()]
            .uuid
            .starts_with("e5913c1e")
    );
    // Line 1 is a stale last-prompt: the leaf is the newest entry.
    assert_eq!(
        path_prefixes(sk, &sk.main_path),
        [
            "664dd794", "f095e45f", "aa6ac045", "6b249596", "e5913c1e", "2c6261a0", "3636a7ba",
            "76cf8fd7", "75fdb5ea", "0fa88fb5"
        ]
    );
    let t = main_transcript(&s, vec![]);
    assert!(t.branch_points.is_empty());
    assert_eq!(prompt_texts(&t).len(), 3);
    assert_eq!(assistant_texts(&t).len(), 4);
}

#[test]
fn later_logical_parent_does_not_create_a_cycle() {
    let s = session(fixture("compact", ids::COMPACT_DIR, ids::COMPACT_CYCLE));
    let sk = &s.skeleton;
    let boundary = &sk.nodes[sk.by_uuid[&uuid(sk, "4c057b73")]];
    // logicalParentUuid points at a later descendant: rejected, nearest preceding entry used.
    assert!(
        sk.nodes[boundary.parent.unwrap()]
            .uuid
            .starts_with("f49773d5")
    );
    let path = path_prefixes(sk, &sk.main_path);
    assert_eq!(path.first().map(String::as_str), Some("7eee0dfd"));
    assert_eq!(path.last().map(String::as_str), Some("3422c256"));
    assert_eq!(path.len(), 13);
    assert!(path.contains(&"4c057b73".to_owned()));
    let t = main_transcript(&s, vec![]);
    assert!(t.branch_points.is_empty());
}

#[test]
fn parent_cycles_are_broken() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"a","parentUuid":"c","message":{"role":"user","content":"a"}}"#,
        r#"{"type":"assistant","uuid":"b","parentUuid":"a","message":{"id":"m1","content":[{"type":"text","text":"b"}]}}"#,
        r#"{"type":"user","uuid":"c","parentUuid":"b","message":{"role":"user","content":"c"}}"#,
    ]);
    let sk = build_skeleton(&[f]);
    assert_eq!(sk.nodes.iter().filter(|n| n.parent.is_none()).count(), 1);
    assert_eq!(sk.main_path.len(), 3);
}

#[test]
fn leaf_uuid_descends_to_the_newest_leaf() {
    // last-prompt points at a system entry; later entries hang below it in two branches.
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"start"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:02Z","message":{"id":"m1","content":[{"type":"text","text":"ok"}]}}"#,
        r#"{"type":"system","subtype":"turn_duration","uuid":"s1","parentUuid":"a1","timestamp":"2026-01-01T00:00:03Z"}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"s1","timestamp":"2026-01-01T00:00:04Z","message":{"role":"user","content":"older branch"}}"#,
        r#"{"type":"user","uuid":"u3","parentUuid":"s1","timestamp":"2026-01-01T00:00:05Z","message":{"role":"user","content":"newer branch"}}"#,
        r#"{"type":"assistant","uuid":"a3","parentUuid":"u3","timestamp":"2026-01-01T00:00:06Z","message":{"id":"m3","content":[{"type":"text","text":"reply"}]}}"#,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"u2","timestamp":"2026-01-01T00:00:07Z","message":{"id":"m2","content":[{"type":"text","text":"late reply"}]}}"#,
        r#"{"type":"last-prompt","leafUuid":"s1"}"#,
    ]);
    let sk = build_skeleton(&[f]);
    // a2 is the newest entry in file order, so the descent goes through u2.
    assert_eq!(
        path_prefixes(&sk, &sk.main_path),
        ["u1", "a1", "s1", "u2", "a2"]
    );

    let basic = fixture("basic", ids::BASIC_DIR, ids::BASIC);
    let sk = build_skeleton(&[basic]);
    // Line 2 is a stale last-prompt; the last valid one points at the final assistant entry.
    assert!(
        sk.nodes[*sk.main_path.last().unwrap()]
            .uuid
            .starts_with("0bd99c42")
    );
}

// ---- branch ----

fn branch_session() -> AssembledSession {
    session(fixture("branch", ids::BRANCH_DIR, ids::BRANCH))
}

#[test]
fn rewind_under_system_satellite_and_regeneration_are_branch_points() {
    let s = branch_session();
    let sk = &s.skeleton;
    let t = main_transcript(&s, vec![]);
    assert_eq!(t.branch_points.len(), 2, "{:#?}", t.branch_points);

    let rewind = &t.branch_points[0];
    assert_eq!(rewind.anchor_key, uuid(sk, "ee434cdd"));
    assert_eq!(rewind.selected_head_id, uuid(sk, "b9a7fd0f"));
    let previews: Vec<&str> = rewind.options.iter().map(|o| o.preview.as_str()).collect();
    assert_eq!(
        previews,
        ["Now add unit tests", "Add property tests instead"]
    );
    assert_eq!(
        rewind
            .options
            .iter()
            .map(|o| o.is_main_line)
            .collect::<Vec<_>>(),
        [false, true]
    );
    assert_eq!(rewind.options[0].reply_count, 1);

    let regen = &t.branch_points[1];
    assert_eq!(regen.anchor_key, uuid(sk, "c43edace"));
    assert_eq!(regen.selected_head_id, uuid(sk, "8a9135b8"));
    let previews: Vec<&str> = regen.options.iter().map(|o| o.preview.as_str()).collect();
    assert_eq!(previews, ["Summary, draft one.", "Summary, draft two."]);

    // Every selected head is a rendered node.
    for bp in &t.branch_points {
        assert!(t.nodes.iter().any(|n| n.id == bp.selected_head_id));
    }
    // Main Line ends at R2 (thinking + text merged into one node).
    assert_eq!(
        assistant_texts(&t).last().map(String::as_str),
        Some("Summary, draft two.")
    );
    assert_eq!(
        sk.nodes[*sk.main_path.last().unwrap()].uuid,
        uuid(sk, "19241985")
    );
}

#[test]
fn verbatim_resend_without_reply_is_hidden_but_never_when_selected() {
    let s = branch_session();
    let sk = &s.skeleton;
    let t = main_transcript(&s, vec![]);
    let resend_anchor = uuid(sk, "83d0281f");
    assert!(
        t.branch_points
            .iter()
            .all(|bp| bp.anchor_key != resend_anchor)
    );
    assert_eq!(
        prompt_texts(&t)
            .iter()
            .filter(|p| p.as_str() == "Run the whole suite")
            .count(),
        1
    );
    // A14d: selecting the unanswered resend keeps it, so the switcher appears.
    let t = main_transcript(
        &s,
        vec![BranchChoice {
            anchor_key: resend_anchor.clone(),
            head_id: uuid(sk, "f1cd0171"),
        }],
    );
    let bp = t
        .branch_points
        .iter()
        .find(|bp| bp.anchor_key == resend_anchor)
        .expect("selected resend keeps its BranchPoint");
    assert_eq!(bp.selected_head_id, uuid(sk, "f1cd0171"));
    assert_eq!(bp.options.len(), 2);
    assert!(
        assistant_texts(&t)
            .last()
            .unwrap()
            .starts_with("Added property-based tests.")
    );
}

#[test]
fn choosing_an_alternative_head_follows_its_newest_leaf() {
    let s = branch_session();
    let sk = &s.skeleton;
    let t = main_transcript(
        &s,
        vec![BranchChoice {
            anchor_key: uuid(sk, "ee434cdd"),
            head_id: uuid(sk, "96ad01af"),
        }],
    );
    let prompts = prompt_texts(&t);
    assert_eq!(
        prompts.last().map(String::as_str),
        Some("Now add unit tests")
    );
    assert!(!prompts.iter().any(|p| p == "Add property tests instead"));
    assert_eq!(
        assistant_texts(&t).last().map(String::as_str),
        Some("Added unit tests.")
    );
    assert_eq!(t.branch_points.len(), 1);
    assert_eq!(t.branch_points[0].selected_head_id, uuid(sk, "96ad01af"));
    // The system satellite between the anchor and the heads stays on the path (hidden).
    let path = codeck_core::assemble::branch::resolve_path(
        sk,
        &[BranchChoice {
            anchor_key: uuid(sk, "ee434cdd"),
            head_id: uuid(sk, "96ad01af"),
        }],
    );
    assert!(path_prefixes(sk, &path).contains(&"02b49efb".to_owned()));

    // Regenerated reply R1: thinking + text fragments merged into one node.
    let t = main_transcript(
        &s,
        vec![BranchChoice {
            anchor_key: uuid(sk, "c43edace"),
            head_id: uuid(sk, "fb25dedc"),
        }],
    );
    let last = t.nodes.last().unwrap();
    assert_eq!(last.id, uuid(sk, "fb25dedc"));
    match &last.body {
        NodeBody::Assistant { blocks, .. } => {
            assert!(
                matches!(&blocks[0], AssistantBlock::Thinking { text } if text == "Draft one.")
            );
            assert!(
                matches!(&blocks[1], AssistantBlock::Text { text } if text == "Summary, draft one.")
            );
        }
        b => panic!("unexpected {b:?}"),
    }

    // Stale / unknown choices are ignored.
    let t = main_transcript(
        &s,
        vec![BranchChoice {
            anchor_key: "nope".into(),
            head_id: uuid(sk, "96ad01af"),
        }],
    );
    assert_eq!(
        assistant_texts(&t).last().map(String::as_str),
        Some("Summary, draft two.")
    );
}

#[test]
fn alternative_head_with_several_leaves_takes_the_newest() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"q"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"m1","content":[{"type":"text","text":"a"}]}}"#,
        r#"{"type":"user","uuid":"h1","parentUuid":"a1","message":{"role":"user","content":"left"}}"#,
        r#"{"type":"assistant","uuid":"x1","parentUuid":"h1","message":{"id":"mx1","content":[{"type":"text","text":"left one"}]}}"#,
        r#"{"type":"assistant","uuid":"x2","parentUuid":"h1","message":{"id":"mx2","content":[{"type":"text","text":"left two"}]}}"#,
        r#"{"type":"user","uuid":"h2","parentUuid":"a1","message":{"role":"user","content":"right"}}"#,
        r#"{"type":"assistant","uuid":"y1","parentUuid":"h2","message":{"id":"my1","content":[{"type":"text","text":"right one"}]}}"#,
        r#"{"type":"last-prompt","leafUuid":"y1"}"#,
    ]);
    let s = session(f);
    let t = main_transcript(
        &s,
        vec![BranchChoice {
            anchor_key: "a1".into(),
            head_id: "h1".into(),
        }],
    );
    assert_eq!(
        assistant_texts(&t).last().map(String::as_str),
        Some("left two")
    );
    // Below the chosen head, the regenerated replies form a second BranchPoint.
    assert_eq!(t.branch_points.len(), 2);
    assert_eq!(t.branch_points[1].anchor_key, "h1");
    assert_eq!(t.branch_points[1].selected_head_id, "x2");
    // A further choice applies below the first one.
    let t = main_transcript(
        &s,
        vec![
            BranchChoice {
                anchor_key: "a1".into(),
                head_id: "h1".into(),
            },
            BranchChoice {
                anchor_key: "h1".into(),
                head_id: "x1".into(),
            },
        ],
    );
    assert_eq!(
        assistant_texts(&t).last().map(String::as_str),
        Some("left one")
    );
}

#[test]
fn tool_result_next_to_hook_attachment_is_not_a_branch() {
    let s = branch_session();
    let sk = &s.skeleton;
    let tool_use = uuid(sk, "47bb8cbc");
    assert_eq!(sk.children[sk.by_uuid[&tool_use]].len(), 2);
    let t = main_transcript(&s, vec![]);
    assert!(
        t.branch_points
            .iter()
            .all(|bp| bp.anchor_key != tool_use && bp.anchor_key != uuid(sk, "73ee2a55"))
    );
}

#[test]
fn main_set_covers_the_main_line_display_set() {
    let s = branch_session();
    let sk = &s.skeleton;
    let set = tree::main_set(sk);
    for p in [
        "30e1f8b9", "64d8360e", "47bb8cbc", "73ee2a55", "9cc6996c", "02b49efb", "b9a7fd0f",
        "f66d761f", "8a9135b8", "19241985",
    ] {
        assert!(set.contains(&uuid(sk, p)), "{p} missing");
    }
    for p in ["96ad01af", "14ea6477", "f1cd0171", "fb25dedc", "a68a2cc0"] {
        assert!(
            !set.contains(&uuid(sk, p)),
            "{p} must not be on the Main Line"
        );
    }

    // Off-path parallel tool results of a merged message are included (basic, lines 25–33).
    let basic = build_skeleton(&[fixture("basic", ids::BASIC_DIR, ids::BASIC)]);
    let set = tree::main_set(&basic);
    for p in [
        "7f17d54c", "0567528b", "a7363696", "fe428371", "a611b656", "9cc6996c",
    ] {
        if basic.nodes.iter().any(|n| n.uuid.starts_with(p)) {
            assert!(set.contains(&uuid(&basic, p)), "{p} missing");
        }
    }
    assert_eq!(
        set.len(),
        basic.nodes.len(),
        "basic has no abandoned branch"
    );
}
