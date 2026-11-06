//! Fixture sanity. Plain serde_json on purpose: no codeck-core APIs, so these tests pin the fixtures
//! themselves. Expected numbers are documented in `tests/fixtures/README.md`.
mod common;

use common::{FIXTURES, SCENARIOS, fixture_root, fixture_root_with_pids, ids};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// (path relative to tests/fixtures, non-empty lines, lines that fail to parse)
const FILES: &[(&str, usize, usize)] = &[
    (
        "basic/projects/-Users-dev-code-lumen-api/86d20d64-9fdd-5e64-aae4-93895b13068d.jsonl",
        77,
        1,
    ),
    (
        "branch/projects/-Users-dev-code-orbit-web/6d4df888-616c-528f-b5b9-674beb27f393.jsonl",
        23,
        0,
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/699c395a-2b04-59eb-bb3a-eb0e0847717b.jsonl",
        13,
        0,
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/6a5f53ed-a977-58c4-a74d-a5e9d4877d22.jsonl",
        14,
        0,
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/d131117c-dd2d-5403-9be8-9b5c492229a9.jsonl",
        11,
        0,
    ),
    (
        "continued/projects/-Users-dev-code-heron-bot/7c1e5a42-93b0-5d8e-a6f1-2b4c8d0e9f13.jsonl",
        149,
        0,
    ),
    (
        "continued/projects/-Users-dev-code-heron-bot/9a3f6d18-2e7c-5b40-8d95-c1e0f7a2b364.jsonl",
        269,
        0,
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli-old/2000072b-1616-53ff-b8dd-0b509f44339e.jsonl",
        5,
        0,
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli-old/d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa.jsonl",
        4,
        0,
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli/2000072b-1616-53ff-b8dd-0b509f44339e.jsonl",
        5,
        0,
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli/d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa.jsonl",
        9,
        0,
    ),
    (
        "fork/projects/-Users-dev-code-tidepool/d3b58148-dbd3-54d8-a4e3-f272df8eac0d.jsonl",
        73,
        0,
    ),
    (
        "fork/projects/-Users-dev-code-tidepool/d6ac0c3a-1415-5f66-bbd9-6cc17c0eb4f5.jsonl",
        73,
        0,
    ),
    (
        "live/projects/-Users-dev-code-orbit-web/45470335-35d2-5185-ad2c-5f6ca8463a06.jsonl",
        2,
        0,
    ),
    (
        "live/projects/-Users-dev-code-orbit-web/b5805323-fcce-58f1-bcf9-40e720af88f4.jsonl",
        3,
        0,
    ),
    (
        "persisted_and_images/projects/-Users-dev-code-lumen-api/3e7603aa-503a-511b-8228-4e9155f1d5e4.jsonl",
        16,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967.jsonl",
        18,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967/subagents/agent-a0rphan000000000001.jsonl",
        2,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967/subagents/agent-a11b19d522e5e3835.jsonl",
        7,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967/subagents/agent-a1e2a06b87a0850ff.jsonl",
        12,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967/subagents/agent-a87b5e24025a157e4.jsonl",
        14,
        0,
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967/subagents/agent-agui-impl-16f075d476ac2e52.jsonl",
        18,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa.jsonl",
        11,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_6e7e94b2-011/agent-a3e22cdf3b6a56db2.jsonl",
        8,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_6e7e94b2-011/agent-ad33e6d9c257a9b69.jsonl",
        8,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_6e7e94b2-011/agent-ad8527df83ea5bfde.jsonl",
        8,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_6e7e94b2-011/journal.jsonl",
        6,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_d83f7287-279/agent-a0423241c80dddf06.jsonl",
        6,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_d83f7287-279/agent-a2a3dee6c7ae88510.jsonl",
        6,
        0,
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa/subagents/workflows/wf_d83f7287-279/journal.jsonl",
        3,
        0,
    ),
];

/// main transcripts: (path, unique uuids, duplicate uuids, human prompts, distinct assistant message ids,
/// tool_use blocks, tool_result errors, tokens [in, out, cache_read, cache_create] counted once per message id)
type FactRow = (
    &'static str,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    [u64; 4],
);
const FACTS: &[FactRow] = &[
    (
        "basic/projects/-Users-dev-code-lumen-api/86d20d64-9fdd-5e64-aae4-93895b13068d.jsonl",
        66,
        1,
        4,
        16,
        18,
        1,
        [51, 5151, 1761742, 33071],
    ),
    (
        "branch/projects/-Users-dev-code-orbit-web/6d4df888-616c-528f-b5b9-674beb27f393.jsonl",
        21,
        0,
        7,
        8,
        1,
        0,
        [40, 804, 160000, 8000],
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/699c395a-2b04-59eb-bb3a-eb0e0847717b.jsonl",
        12,
        0,
        4,
        4,
        0,
        0,
        [20, 480, 80000, 4000],
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/6a5f53ed-a977-58c4-a74d-a5e9d4877d22.jsonl",
        13,
        0,
        4,
        4,
        0,
        0,
        [20, 480, 80000, 4000],
    ),
    (
        "compact/projects/-Users-dev-code-tidepool/d131117c-dd2d-5403-9be8-9b5c492229a9.jsonl",
        10,
        0,
        3,
        4,
        0,
        0,
        [20, 480, 80000, 4000],
    ),
    (
        "continued/projects/-Users-dev-code-heron-bot/7c1e5a42-93b0-5d8e-a6f1-2b4c8d0e9f13.jsonl",
        94,
        0,
        2,
        10,
        11,
        3,
        [20, 2941, 588946, 44668],
    ),
    (
        "continued/projects/-Users-dev-code-heron-bot/9a3f6d18-2e7c-5b40-8d95-c1e0f7a2b364.jsonl",
        208,
        0,
        5,
        22,
        21,
        3,
        [46, 8765, 1542693, 56935],
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli-old/2000072b-1616-53ff-b8dd-0b509f44339e.jsonl",
        4,
        0,
        2,
        2,
        0,
        0,
        [10, 240, 40000, 2000],
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli-old/d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa.jsonl",
        4,
        0,
        2,
        2,
        0,
        0,
        [10, 240, 40000, 2000],
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli/2000072b-1616-53ff-b8dd-0b509f44339e.jsonl",
        4,
        0,
        2,
        2,
        0,
        0,
        [10, 240, 40000, 2000],
    ),
    (
        "copies/projects/-Users-dev-code-kestrel-cli/d7b7f7c0-01f9-5dc2-b356-815f6c8d6cfa.jsonl",
        8,
        0,
        4,
        4,
        0,
        0,
        [20, 480, 80000, 4000],
    ),
    (
        "fork/projects/-Users-dev-code-tidepool/d3b58148-dbd3-54d8-a4e3-f272df8eac0d.jsonl",
        53,
        0,
        2,
        5,
        3,
        0,
        [13, 6337, 152651, 44466],
    ),
    (
        "fork/projects/-Users-dev-code-tidepool/d6ac0c3a-1415-5f66-bbd9-6cc17c0eb4f5.jsonl",
        53,
        0,
        2,
        5,
        3,
        0,
        [13, 6337, 152651, 44466],
    ),
    (
        "live/projects/-Users-dev-code-orbit-web/45470335-35d2-5185-ad2c-5f6ca8463a06.jsonl",
        2,
        0,
        1,
        1,
        0,
        0,
        [5, 120, 20000, 1000],
    ),
    (
        "live/projects/-Users-dev-code-orbit-web/b5805323-fcce-58f1-bcf9-40e720af88f4.jsonl",
        2,
        0,
        1,
        1,
        0,
        0,
        [5, 120, 20000, 1000],
    ),
    (
        "persisted_and_images/projects/-Users-dev-code-lumen-api/3e7603aa-503a-511b-8228-4e9155f1d5e4.jsonl",
        15,
        0,
        2,
        6,
        3,
        0,
        [1181, 1527, 631103, 7634],
    ),
    (
        "subagents/projects/-Users-dev-code-tidepool/ee98ea57-9b48-56b0-9860-5c8105c7f967.jsonl",
        17,
        0,
        1,
        4,
        3,
        0,
        [10, 13244, 346381, 100767],
    ),
    (
        "workflow/projects/-Users-dev-code-kestrel-cli/418be169-3e5e-5397-b9f9-862014ebf2fa.jsonl",
        10,
        0,
        1,
        3,
        2,
        0,
        [9, 16001, 86971, 170527],
    ),
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn files_in(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(root, &mut v);
    v.sort();
    v
}

fn rel(p: &Path) -> String {
    p.strip_prefix(FIXTURES)
        .unwrap()
        .to_string_lossy()
        .to_string()
}

/// Parse a jsonl file: (entries with their 1-based line number, failed line numbers).
fn read_jsonl(p: &Path) -> (Vec<(usize, Value)>, Vec<usize>) {
    let text = fs::read_to_string(p).unwrap();
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(v) => ok.push((i + 1, v)),
            Err(_) => bad.push(i + 1),
        }
    }
    (ok, bad)
}

fn session_file(scn: &str, dir: &str, sid: &str) -> PathBuf {
    Path::new(FIXTURES)
        .join(scn)
        .join("projects")
        .join(dir)
        .join(format!("{sid}.jsonl"))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn blocks(v: &Value) -> Vec<&Value> {
    match v.pointer("/message/content") {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => vec![],
    }
}

fn is_human(v: &Value) -> bool {
    if s(v, "type") != "user" || v["isMeta"] == true || v["isCompactSummary"] == true {
        return false;
    }
    if let Some(k) = v.pointer("/origin/kind").and_then(Value::as_str)
        && k != "human"
    {
        return false;
    }
    match v.pointer("/message/content") {
        Some(Value::String(t)) => {
            !t.starts_with("<local-command")
                && !t.starts_with("<system-reminder")
                && !t.starts_with("<teammate-message")
                && !t.starts_with("Another Claude session sent a message:")
        }
        Some(Value::Array(a)) => {
            let ty = |b: &Value| s(b, "type").to_string();
            a.iter().any(|b| ty(b) == "text" || ty(b) == "image")
                && !a.iter().any(|b| ty(b) == "tool_result")
        }
        _ => false,
    }
}

struct Facts {
    uuids: usize,
    dups: usize,
    prompts: usize,
    amids: usize,
    tool_uses: usize,
    errs: usize,
    tokens: [u64; 4],
}

fn facts(entries: &[(usize, Value)]) -> Facts {
    let mut seen = HashSet::new();
    let mut dups = 0;
    let mut prompts = 0;
    let mut tool_uses = 0;
    let mut errs = 0;
    let mut per_msg: HashMap<String, [u64; 4]> = HashMap::new();
    for (_, v) in entries {
        if let Some(u) = v.get("uuid").and_then(Value::as_str)
            && !seen.insert(u.to_string())
        {
            dups += 1;
        }
        if is_human(v) {
            prompts += 1;
        }
        if s(v, "type") == "assistant"
            && let Some(id) = v.pointer("/message/id").and_then(Value::as_str)
        {
            let e = per_msg.entry(id.to_string()).or_insert([0; 4]);
            for (i, k) in [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .iter()
            .enumerate()
            {
                let n = v
                    .pointer(&format!("/message/usage/{k}"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                e[i] = e[i].max(n);
            }
        }
        for b in blocks(v) {
            match s(b, "type") {
                "tool_use" => tool_uses += 1,
                "tool_result" if b["is_error"] == true => errs += 1,
                _ => {}
            }
        }
    }
    let mut tokens = [0u64; 4];
    for t in per_msg.values() {
        for i in 0..4 {
            tokens[i] += t[i];
        }
    }
    Facts {
        uuids: seen.len(),
        dups,
        prompts,
        amids: per_msg.len(),
        tool_uses,
        errs,
        tokens,
    }
}

fn by_uuid(entries: &[(usize, Value)]) -> HashMap<&str, &Value> {
    let mut m = HashMap::new();
    for (_, v) in entries {
        if let Some(u) = v.get("uuid").and_then(Value::as_str) {
            m.entry(u).or_insert(v);
        }
    }
    m
}

fn find_text<'a>(entries: &'a [(usize, Value)], text: &str) -> &'a Value {
    entries
        .iter()
        .map(|(_, v)| v)
        .find(|v| v.pointer("/message/content").and_then(Value::as_str) == Some(text))
        .unwrap_or_else(|| panic!("no entry with text {text:?}"))
}

fn children_of<'a>(entries: &'a [(usize, Value)], parent: &str) -> Vec<&'a Value> {
    entries
        .iter()
        .map(|(_, v)| v)
        .filter(|v| s(v, "parentUuid") == parent)
        .collect()
}

// ---------------------------------------------------------------------------------------------

#[test]
fn every_scenario_parses_with_expected_counts() {
    let mut expected: BTreeMap<&str, (usize, usize)> =
        FILES.iter().map(|&(p, l, f)| (p, (l, f))).collect();
    let mut total = 0u64;
    for scn in SCENARIOS {
        let mut all = Vec::new();
        walk(&Path::new(FIXTURES).join(scn), &mut all);
        for p in all {
            total += fs::metadata(&p).unwrap().len();
            let r = rel(&p);
            if r.ends_with(".jsonl") {
                let (ok, bad) = read_jsonl(&p);
                let want = expected
                    .remove(r.as_str())
                    .unwrap_or_else(|| panic!("{r} is not in FILES"));
                assert_eq!((ok.len() + bad.len(), bad.len()), want, "{r}");
            } else if r.ends_with(".json") {
                serde_json::from_str::<Value>(&fs::read_to_string(&p).unwrap())
                    .unwrap_or_else(|e| panic!("{r}: {e}"));
            }
        }
    }
    assert!(expected.is_empty(), "listed but missing: {expected:?}");
    assert!(total < 2_000_000, "fixtures are {total} bytes");
}

#[test]
fn main_transcript_facts_match_readme() {
    for &(path, uuids, dups, prompts, amids, tool_uses, errs, tokens) in FACTS {
        let (entries, _) = read_jsonl(&Path::new(FIXTURES).join(path));
        let f = facts(&entries);
        assert_eq!(
            (
                f.uuids,
                f.dups,
                f.prompts,
                f.amids,
                f.tool_uses,
                f.errs,
                f.tokens
            ),
            (uuids, dups, prompts, amids, tool_uses, errs, tokens),
            "{path}"
        );
    }
}

#[test]
fn nothing_sensitive_in_fixtures() {
    let mut all = Vec::new();
    walk(Path::new(FIXTURES), &mut all);
    for p in all {
        if p.file_name()
            .is_some_and(|n| n == "README.md" || n == ".DS_Store")
        {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap_or_default().to_lowercase();
        let user = std::env::var("USER").unwrap_or_default().to_lowercase();
        let mut bad = vec!["@gmail", "@live", "api_key", "apikey", "password"];
        if user.len() > 2 {
            bad.push(&user);
        }
        // Extra private terms (e.g. employer or client names) are kept out of the repo:
        // CODECK_FIXTURE_DENYLIST="name1,name2" cargo test -p codeck-core --test fixtures_sanity
        let extra = std::env::var("CODECK_FIXTURE_DENYLIST")
            .unwrap_or_default()
            .to_lowercase();
        bad.extend(extra.split(',').map(str::trim).filter(|t| t.len() > 2));
        for b in bad {
            assert!(!text.contains(b), "{} contains {b}", rel(&p));
        }
        for (i, _) in text.match_indices("/users/") {
            assert!(
                text[i..].starts_with("/users/dev"),
                "{} has a non-dev home path",
                rel(&p)
            );
        }
    }
}

#[test]
fn basic_has_every_irregular_shape() {
    let p = session_file("basic", ids::BASIC_DIR, ids::BASIC);
    let (entries, bad) = read_jsonl(&p);
    assert_eq!(bad, vec![66], "one corrupt (truncated) line");
    let types: Vec<&str> = entries.iter().map(|(_, v)| s(v, "type")).collect();
    for t in [
        "hologram",
        "ai-title",
        "custom-title",
        "last-prompt",
        "permission-mode",
        "mode",
        "queue-operation",
        "file-history-snapshot",
        "agent-name",
    ] {
        assert!(types.contains(&t), "missing entry type {t}");
    }
    assert!(
        entries
            .iter()
            .any(|(_, v)| blocks(v).iter().any(|b| s(b, "type") == "hologram_block")),
        "unknown block type"
    );
    assert!(
        entries
            .iter()
            .any(|(_, v)| s(v, "subtype") == "future_notice"),
        "unknown system subtype"
    );
    // line 2 is a stale last-prompt (its leaf is not in the file); the last one resolves
    let uu = by_uuid(&entries);
    let leaves: Vec<&str> = entries
        .iter()
        .filter(|(_, v)| s(v, "type") == "last-prompt")
        .map(|(_, v)| s(v, "leafUuid"))
        .collect();
    assert_eq!(leaves.len(), 2);
    assert!(!uu.contains_key(leaves[0]) && uu.contains_key(leaves[1]));
    // tool kinds
    let mut names = HashSet::new();
    for (_, v) in &entries {
        for b in blocks(v) {
            if s(b, "type") == "tool_use" {
                names.insert(s(b, "name").to_string());
            }
        }
    }
    for n in [
        "Bash",
        "Read",
        "Edit",
        "Write",
        "Grep",
        "Skill",
        "AskUserQuestion",
        "TaskCreate",
        "TaskUpdate",
        "WebFetch",
        "TodoWrite",
    ] {
        assert!(names.contains(n), "missing tool {n}");
    }
    assert!(names.iter().any(|n| n.starts_with("mcp__")), "mcp tool");
    // failed tool call carries ANSI escapes; split assistant fragments repeat the same usage
    let failed = entries
        .iter()
        .flat_map(|(_, v)| blocks(v))
        .find(|b| b["is_error"] == true)
        .expect("failed tool_result");
    assert!(failed["content"].as_str().unwrap().contains('\u{1b}'));
    let mut frag_usage: HashMap<&str, Vec<&Value>> = HashMap::new();
    for (_, v) in &entries {
        if s(v, "type") == "assistant" {
            frag_usage
                .entry(s(&v["message"], "id"))
                .or_default()
                .push(&v["message"]["usage"]);
        }
    }
    assert!(
        frag_usage
            .values()
            .any(|u| u.len() >= 3 && u.windows(2).all(|w| w[0] == w[1]))
    );
}

#[test]
fn branch_graph_shapes() {
    let (entries, bad) = read_jsonl(&session_file("branch", ids::BRANCH_DIR, ids::BRANCH));
    assert!(bad.is_empty());
    // rewind under a system satellite: two prompts share the same system parent
    let u3a = find_text(&entries, "Now add unit tests");
    let u3b = find_text(&entries, "Add property tests instead");
    assert_eq!(u3a["parentUuid"], u3b["parentUuid"]);
    let uu = by_uuid(&entries);
    assert_eq!(s(uu[s(u3a, "parentUuid")], "type"), "system");
    // verbatim resend with no reply: identical text, same parent, the first has no children at all
    let same: Vec<&Value> = entries
        .iter()
        .map(|(_, v)| v)
        .filter(|v| {
            v.pointer("/message/content").and_then(Value::as_str) == Some("Run the whole suite")
        })
        .collect();
    assert_eq!(same.len(), 2);
    assert_eq!(same[0]["parentUuid"], same[1]["parentUuid"]);
    assert!(children_of(&entries, s(same[0], "uuid")).is_empty());
    assert!(!children_of(&entries, s(same[1], "uuid")).is_empty());
    // regenerated assistant message: two assistant children with different message ids under one prompt
    let u5 = find_text(&entries, "Summarize the result");
    let kids = children_of(&entries, s(u5, "uuid"));
    assert_eq!(kids.len(), 2);
    assert_ne!(kids[0]["message"]["id"], kids[1]["message"]["id"]);
    // a tool_use fragment has two children (tool_result + hook attachment) and that is NOT a branch case
    let tu = entries
        .iter()
        .map(|(_, v)| v)
        .find(|v| blocks(v).iter().any(|b| s(b, "type") == "tool_use"))
        .unwrap();
    let kinds: HashSet<&str> = children_of(&entries, s(tu, "uuid"))
        .iter()
        .map(|v| s(v, "type"))
        .collect();
    assert_eq!(kinds, HashSet::from(["user", "attachment"]));
    // the last-prompt leaf is the newest regenerated message
    let lp = entries
        .iter()
        .find(|(_, v)| s(v, "type") == "last-prompt")
        .unwrap();
    assert_eq!(
        uu[s(&lp.1, "leafUuid")]["message"]["content"][0]["text"],
        "Summary, draft two."
    );
}

#[test]
fn compact_cases() {
    let load = |sid| read_jsonl(&session_file("compact", ids::COMPACT_DIR, sid)).0;
    // (b) sibling: boundary.logicalParentUuid exists and the `/compact` prompt hangs off the same entry
    let e = load(ids::COMPACT_SIBLING);
    let b = e
        .iter()
        .map(|(_, v)| v)
        .find(|v| s(v, "subtype") == "compact_boundary")
        .unwrap();
    assert!(b["parentUuid"].is_null());
    let lp = s(b, "logicalParentUuid");
    assert!(by_uuid(&e).contains_key(lp));
    let cmd = e
        .iter()
        .map(|(_, v)| v)
        .find(|v| {
            v.pointer("/message/content")
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with("<command-name>/compact"))
        })
        .unwrap();
    assert_eq!(s(cmd, "parentUuid"), lp);
    let sum = children_of(&e, s(b, "uuid"));
    assert_eq!(sum.len(), 1);
    assert_eq!(sum[0]["isCompactSummary"], true);
    // (a) missing: logicalParentUuid is not in the file
    let e = load(ids::COMPACT_MISSING);
    let b = e
        .iter()
        .map(|(_, v)| v)
        .find(|v| s(v, "subtype") == "compact_boundary")
        .unwrap();
    assert!(b["parentUuid"].is_null());
    assert!(!by_uuid(&e).contains_key(s(b, "logicalParentUuid")));
    // cycle: logicalParentUuid names a LATER entry that descends from the boundary
    let e = load(ids::COMPACT_CYCLE);
    let pos = |u: &str| e.iter().position(|(_, v)| s(v, "uuid") == u).unwrap();
    let b = e
        .iter()
        .map(|(_, v)| v)
        .find(|v| s(v, "subtype") == "compact_boundary")
        .unwrap();
    let uu = by_uuid(&e);
    let lp = s(b, "logicalParentUuid");
    assert!(pos(lp) > pos(s(b, "uuid")));
    let mut cur = lp;
    let mut reaches_boundary = false;
    for _ in 0..e.len() {
        match uu.get(cur).map(|v| s(v, "parentUuid")) {
            Some(p) if !p.is_empty() => {
                if p == s(b, "uuid") {
                    reaches_boundary = true;
                    break;
                }
                cur = p;
            }
            _ => break,
        }
    }
    assert!(
        reaches_boundary,
        "logical parent must descend from the boundary (cycle)"
    );
}

#[test]
fn fork_pair() {
    let (origin, _) = read_jsonl(&session_file("fork", ids::FORK_DIR, ids::FORK_ORIGIN));
    let (child, _) = read_jsonl(&session_file("fork", ids::FORK_DIR, ids::FORK_CHILD));
    let ou = by_uuid(&origin);
    let inherited: Vec<&Value> = child
        .iter()
        .map(|(_, v)| v)
        .filter(|v| v.get("forkedFrom").is_some())
        .collect();
    assert_eq!(inherited.len(), 51);
    for v in &inherited {
        assert_eq!(s(&v["forkedFrom"], "sessionId"), ids::FORK_ORIGIN);
        assert_eq!(s(&v["forkedFrom"], "messageUuid"), s(v, "uuid"));
        assert!(
            ou.contains_key(s(v, "uuid")),
            "inherited uuid exists in the origin"
        );
    }
    assert!(origin.iter().all(|(_, v)| v.get("forkedFrom").is_none()));
    // both continue from the same fork point with their own prompt
    let own_o = find_text(
        &origin,
        "Which of the three options is the simplest to ship?",
    );
    let own_c = find_text(&child, "Try the daemon approach instead and compare");
    assert_eq!(own_o["parentUuid"], own_c["parentUuid"]);
    assert!(ou.contains_key(s(own_c, "parentUuid")));
    assert!(
        child
            .iter()
            .any(|(_, v)| s(v, "customTitle").ends_with("(Branch)"))
    );
}

#[test]
fn continued_pair() {
    let path = |sid| session_file("continued", ids::CONTINUED_DIR, sid);
    let (old, _) = read_jsonl(&path(ids::CONTINUED_OLD));
    let (new, _) = read_jsonl(&path(ids::CONTINUED_NEW));
    // the old one ends its conversation with `continued-in`, followed only by uuid-less lines
    let at = old
        .iter()
        .position(|(_, v)| s(v, "type") == "continued-in")
        .unwrap();
    assert_eq!(s(&old[at].1, "continuedInSessionId"), ids::CONTINUED_NEW);
    assert!(old[at..].iter().all(|(_, v)| v.get("uuid").is_none()));
    // every uuid-bearing entry is copied (same uuid and message id; a few parents differ), no `forkedFrom`
    let nu = by_uuid(&new);
    let copied: Vec<&Value> = old
        .iter()
        .map(|(_, v)| v)
        .filter(|v| v.get("uuid").is_some())
        .collect();
    assert_eq!(copied.len(), 94);
    for v in &copied {
        let c = nu[s(v, "uuid")];
        assert_eq!(c["message"]["id"], v["message"]["id"]);
    }
    assert!(new.iter().all(|(_, v)| v.get("forkedFrom").is_none()));
    // its own entries come after the copy, marked as a background Session
    let own: Vec<&Value> = new
        .iter()
        .map(|(_, v)| v)
        .filter(|v| v.get("uuid").is_some() && !copied.iter().any(|c| c["uuid"] == v["uuid"]))
        .collect();
    assert_eq!(own.len(), 114);
    assert!(own.iter().all(|v| s(v, "sessionKind") == "bg"));
}

#[test]
fn copies_layout_and_symlink() {
    let root = fixture_root("copies");
    let proj = root.path().join("projects");
    let f = |dir: &str, sid: &str| fs::read(proj.join(dir).join(format!("{sid}.jsonl"))).unwrap();
    assert_eq!(
        f(ids::COPIES_DIR, ids::COPIES_IDENTICAL),
        f(ids::COPIES_DIR_OLD, ids::COPIES_IDENTICAL)
    );
    let big = f(ids::COPIES_DIR, ids::COPIES_SUPERSET);
    let small = f(ids::COPIES_DIR_OLD, ids::COPIES_SUPERSET);
    assert!(
        big.len() > small.len() && big.starts_with(&small),
        "-old is a byte prefix of the superset"
    );
    let link = proj.join(ids::COPIES_LINK);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(proj.join(ids::COPIES_DIR)).unwrap()
    );
    assert!(
        link.join(format!("{}.jsonl", ids::COPIES_SUPERSET))
            .is_file()
    );
}

#[test]
fn subagent_linking_inputs() {
    let root = fixture_root("subagents");
    let sdir = root
        .path()
        .join("projects")
        .join(ids::SUBAGENTS_DIR)
        .join(ids::SUBAGENTS)
        .join("subagents");
    let (main, _) = read_jsonl(&session_file(
        "subagents",
        ids::SUBAGENTS_DIR,
        ids::SUBAGENTS,
    ));
    let tool_uses: HashSet<String> = main
        .iter()
        .flat_map(|(_, v)| blocks(v))
        .filter(|b| s(b, "type") == "tool_use")
        .map(|b| s(b, "id").to_string())
        .collect();
    let results: Vec<&Value> = main
        .iter()
        .filter_map(|(_, v)| v.get("toolUseResult"))
        .filter(|t| t.is_object())
        .collect();
    let meta = |id: &str| -> Value {
        serde_json::from_str(
            &fs::read_to_string(sdir.join(format!("agent-{id}.meta.json"))).unwrap(),
        )
        .unwrap()
    };
    // sync: meta.toolUseId is a main tool_use; result is `completed` with the agentId
    let m = meta("a11b19d522e5e3835");
    assert!(tool_uses.contains(s(&m, "toolUseId")));
    assert!(
        results
            .iter()
            .any(|t| s(t, "status") == "completed" && s(t, "agentId") == "a11b19d522e5e3835")
    );
    // async: `async_launched` result + a task-notification prompt carrying task-id and tool-use-id
    let m = meta("a87b5e24025a157e4");
    assert!(
        results
            .iter()
            .any(|t| s(t, "status") == "async_launched" && s(t, "agentId") == "a87b5e24025a157e4")
    );
    let note = main
        .iter()
        .map(|(_, v)| v)
        .find(|v| s(&v["origin"], "kind") == "task-notification")
        .unwrap();
    let text = note["message"]["content"].as_str().unwrap();
    assert!(text.contains("<task-id>a87b5e24025a157e4</task-id>"));
    assert!(text.contains(&format!(
        "<tool-use-id>{}</tool-use-id>",
        s(&m, "toolUseId")
    )));
    // teammate: no toolUseId; linked by meta.name == toolUseResult.name
    let m = meta("agui-impl-16f075d476ac2e52");
    assert!(m.get("toolUseId").is_none());
    assert!(
        results
            .iter()
            .any(|t| s(t, "status") == "teammate_spawned" && s(t, "name") == s(&m, "name"))
    );
    // nested: parentAgentId is the teammate and its toolUseId is a tool_use inside the teammate's file
    let n = meta("a1e2a06b87a0850ff");
    assert_eq!(s(&n, "parentAgentId"), "agui-impl-16f075d476ac2e52");
    let (parent, _) = read_jsonl(&sdir.join("agent-agui-impl-16f075d476ac2e52.jsonl"));
    assert!(
        parent
            .iter()
            .flat_map(|(_, v)| blocks(v))
            .any(|b| s(b, "type") == "tool_use" && s(b, "id") == s(&n, "toolUseId"))
    );
    assert!(!tool_uses.contains(s(&n, "toolUseId")));
    // orphan: toolUseId matches no tool_use in any file
    let o = meta("a0rphan000000000001");
    let needle = s(&o, "toolUseId").to_string();
    for p in files_in(&sdir).into_iter().chain([session_file(
        "subagents",
        ids::SUBAGENTS_DIR,
        ids::SUBAGENTS,
    )]) {
        if p.to_string_lossy().ends_with(".jsonl") {
            assert!(
                !fs::read_to_string(&p).unwrap().contains(&needle),
                "{}",
                rel_or(&p)
            );
        }
    }
    // every agent transcript is a sidechain with its own agentId
    for p in files_in(&sdir)
        .iter()
        .filter(|p| p.to_string_lossy().ends_with(".jsonl"))
    {
        let id = p
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches("agent-")
            .to_string();
        for (_, v) in read_jsonl(p).0 {
            assert_eq!(v["isSidechain"], true);
            assert_eq!(s(&v, "agentId"), id);
        }
    }
}

fn rel_or(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[test]
fn workflow_runs() {
    let sdir = Path::new(FIXTURES)
        .join("workflow/projects")
        .join(ids::WORKFLOW_DIR)
        .join(ids::WORKFLOW);
    let (main, _) = read_jsonl(&session_file("workflow", ids::WORKFLOW_DIR, ids::WORKFLOW));
    let run_ids: Vec<&str> = main
        .iter()
        .filter_map(|(_, v)| v.get("toolUseResult"))
        .filter(|t| t.is_object())
        .map(|t| s(t, "runId"))
        .filter(|r| !r.is_empty())
        .collect();
    assert_eq!(run_ids, vec!["wf_6e7e94b2-011", "wf_d83f7287-279"]);
    // run A: json + journal + agents
    let json: Value = serde_json::from_str(
        &fs::read_to_string(sdir.join("workflows/wf_6e7e94b2-011.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(s(&json, "status"), "completed");
    assert_eq!(
        json["workflowProgress"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| s(x, "type") == "workflow_phase")
            .count(),
        2
    );
    let agents: Vec<&str> = json["workflowProgress"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| s(x, "type") == "workflow_agent")
        .map(|x| s(x, "agentId"))
        .collect();
    assert_eq!(agents.len(), 3);
    let adir = sdir.join("subagents/workflows/wf_6e7e94b2-011");
    for a in &agents {
        assert!(
            adir.join(format!("agent-{a}.jsonl")).is_file()
                && adir.join(format!("agent-{a}.meta.json")).is_file()
        );
    }
    let (journal, _) = read_jsonl(&adir.join("journal.jsonl"));
    assert_eq!(
        journal
            .iter()
            .filter(|(_, v)| s(v, "type") == "started")
            .count(),
        3
    );
    assert_eq!(
        journal
            .iter()
            .filter(|(_, v)| s(v, "type") == "result")
            .count(),
        3
    );
    // run B: still running, so no json: journal (2 started, 1 result) + 2 agents only
    assert!(!sdir.join("workflows/wf_d83f7287-279.json").exists());
    let bdir = sdir.join("subagents/workflows/wf_d83f7287-279");
    let (journal, _) = read_jsonl(&bdir.join("journal.jsonl"));
    assert_eq!(
        journal
            .iter()
            .filter(|(_, v)| s(v, "type") == "started")
            .count(),
        2
    );
    assert_eq!(
        journal
            .iter()
            .filter(|(_, v)| s(v, "type") == "result")
            .count(),
        1
    );
    let agent_files = files_in(&bdir)
        .iter()
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("agent-")
                && p.to_string_lossy().ends_with(".jsonl")
        })
        .count();
    assert_eq!(agent_files, 2);
}

#[test]
fn persisted_outputs_and_images() {
    let root = fixture_root("persisted_and_images");
    let sdir = root
        .path()
        .join("projects")
        .join(ids::PERSISTED_DIR)
        .join(ids::PERSISTED);
    let (entries, bad) = read_jsonl(&session_file(
        "persisted_and_images",
        ids::PERSISTED_DIR,
        ids::PERSISTED,
    ));
    assert!(bad.is_empty());
    let mut names = Vec::new();
    for (_, v) in &entries {
        for b in blocks(v) {
            if let Some(t) = b["content"]
                .as_str()
                .filter(|t| t.starts_with("<persisted-output>"))
            {
                let base = t
                    .split("tool-results/")
                    .nth(1)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_string();
                assert_eq!(
                    v["toolUseResult"]["persistedOutputPath"]
                        .as_str()
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap(),
                    base
                );
                names.push(base);
            }
        }
    }
    assert_eq!(names, vec!["b8e6n2j5e.txt", "zz9missing1.txt"]);
    assert!(sdir.join("tool-results/b8e6n2j5e.txt").is_file());
    assert!(
        !sdir.join("tool-results/zz9missing1.txt").exists(),
        "second offload points at a missing file"
    );
    // both image shapes: top-level image block in a prompt, and image inside a tool_result
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let text = fs::read_to_string(session_file(
        "persisted_and_images",
        ids::PERSISTED_DIR,
        ids::PERSISTED,
    ))
    .unwrap();
    assert_eq!(
        text.matches(png).count(),
        3,
        "prompt image + tool_result image + toolUseResult.file.base64"
    );
    assert!(
        entries
            .iter()
            .any(|(_, v)| s(v, "type") == "user"
                && blocks(v).iter().any(|b| s(b, "type") == "image"))
    );
}

#[test]
fn live_sessions_render_for_this_process() {
    let (root, pids) = fixture_root_with_pids("live");
    let pids = pids.unwrap();
    let dir = root.path().join("sessions");
    let read = |pid: u32| -> Value {
        serde_json::from_str(&fs::read_to_string(dir.join(format!("{pid}.json"))).unwrap()).unwrap()
    };
    let me = read(pids.pid);
    assert_eq!(me["pid"], pids.pid);
    assert_eq!(s(&me, "sessionId"), ids::LIVE_BUSY);
    assert_eq!(s(&me, "status"), "busy");
    assert_eq!(s(&me, "procStart"), common::proc_start(pids.pid));
    assert!(
        s(&me, "procStart").len() >= 24,
        "ctime-style string: {}",
        s(&me, "procStart")
    );
    let parent = read(pids.ppid);
    assert_eq!(
        (s(&parent, "sessionId"), s(&parent, "status")),
        (ids::LIVE_SHELL, "shell")
    );
    assert_eq!(s(&parent, "procStart"), common::proc_start(pids.ppid));
    let dead = read(pids.dead_pid);
    assert_eq!(
        (s(&dead, "sessionId"), s(&dead, "status")),
        (ids::LIVE_DEAD, "idle")
    );
    let reused = read(1);
    assert_eq!(s(&reused, "sessionId"), ids::LIVE_REUSED);
    assert_ne!(s(&reused, "procStart"), common::proc_start(1));
    // `.key` files sit next to the json files and must be ignored by scanners; no template is left behind
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with(".key")));
    assert!(!names.iter().any(|n| n.ends_with(".template")));
    // transcripts for the two live sessions exist
    for sid in [ids::LIVE_BUSY, ids::LIVE_SHELL] {
        assert!(
            root.path()
                .join("projects")
                .join(ids::LIVE_DIR)
                .join(format!("{sid}.jsonl"))
                .is_file()
        );
    }
}

#[test]
fn fixture_root_copies_every_scenario() {
    for scn in SCENARIOS {
        let root = fixture_root(scn);
        assert!(root.path().join("projects").is_dir(), "{scn}");
    }
}
