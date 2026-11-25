//! Code Mode assembly, counting, search, jump, images and fallback display.
mod support;

use std::path::Path;

use support::fixtures::{fixture_dir, ids};
use support::{call, from_lines, load, main_t, open, tool_calls};
use transcript::assemble::{SessionSidecar, SubagentMetaFile, assemble, content, find, summarize};
use transcript::model::{
    AssistantBlock, FileRole, FindLocation, FindRequest, ImageRequest, NodeBody, ToolCall,
    ToolDetailRequest, ToolPart, Transcript, TranscriptScope,
};

/// uuid of the `n`-th entry of the code_mode fixture (see its README section).
fn cm(n: u32) -> String {
    format!("c0de{n:04}-0000-4000-8000-{n:012}")
}

fn code_of<'a>(t: &'a Transcript, id: &str) -> &'a transcript::model::CodeRun {
    call(t, id).code.as_ref().expect("REPL call has a code run")
}

/// The calls the code of `REPL` call `id` made: the calls naming it in `via_code`.
fn made_by<'a>(t: &'a Transcript, id: &str) -> Vec<&'a ToolCall> {
    tool_calls(t)
        .into_iter()
        .filter(|c| c.via_code.as_deref() == Some(id))
        .collect()
}

#[test]
fn code_mode_lists_the_calls_the_code_made_after_it() {
    let s = open("code_mode", ids::CODE_MODE_DIR, ids::CODE_MODE);
    let t = main_t(&s, true);
    // The virtual entries are not nodes of their own, and Poll is not shown.
    let virtual_ids: Vec<String> = (7..=14).chain(19..=22).chain(29..=32).map(cm).collect();
    assert!(t.nodes.iter().all(|n| !virtual_ids.contains(&n.id)));
    assert!(tool_calls(&t).iter().all(|c| c.name != "Poll"));
    let names = |id: &str| -> Vec<(String, String)> {
        made_by(&t, id)
            .iter()
            .map(|c| (c.tool_use_id.clone(), c.name.clone()))
            .collect()
    };

    // Run 1: four calls, result delivered in a poll_events attachment.
    let r1 = code_of(&t, "toolu_c0de_r1");
    let c1 = made_by(&t, "toolu_c0de_r1");
    assert_eq!(r1.call_count, 4);
    assert_eq!(
        names("toolu_c0de_r1"),
        [
            ("repl_c0de-v1", "Bash"),
            ("repl_c0de-v2", "Edit"),
            ("repl_c0de-v3", "Edit"),
            ("repl_c0de-v4", "Bash")
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    let o1 = r1.outcome.as_ref().expect("outcome");
    assert!(!o1.is_error);
    assert_eq!(
        o1.text,
        "{ edited: [ 'src/client/retry.rs', 'src/middleware/upstream.rs' ], exit: 0 }"
    );
    assert_eq!(o1.duration_ms, Some(9123.0));
    let rg = c1[0].result.as_ref().unwrap();
    assert_eq!(rg.text, "src/client/retry.rs\nsrc/middleware/upstream.rs");
    assert!(rg.extra_json.as_deref().unwrap().contains("\"stdout\""));
    assert!(
        c1[1]
            .result
            .as_ref()
            .unwrap()
            .extra_json
            .as_deref()
            .unwrap()
            .contains("structuredPatch")
    );

    // Run 2: the code threw; the virtual REPL error pair is not one of its calls.
    let r2 = code_of(&t, "toolu_c0de_r2");
    assert_eq!(
        names("toolu_c0de_r2"),
        [("repl_c0de-v5".to_owned(), "Read".to_owned())]
    );
    assert_eq!(
        made_by(&t, "toolu_c0de_r2")[0]
            .result
            .as_ref()
            .unwrap()
            .text,
        "     1\t[server]\n     2\tport = 8080"
    );
    let o2 = r2.outcome.as_ref().unwrap();
    assert!(o2.is_error);
    assert_eq!(
        o2.text,
        "TypeError: Cannot read properties of undefined (reading 'max_attempts')\n    at repl:2:11"
    );

    // Run 3: delivered as the Poll result (the quoted event is ignored); a call inside failed.
    let r3 = code_of(&t, "toolu_c0de_r3");
    let c3 = made_by(&t, "toolu_c0de_r3");
    assert_eq!((c3.len(), r3.call_count), (2, 2));
    assert!(c3[0].result.as_ref().unwrap().is_error);
    let o3 = r3.outcome.as_ref().unwrap();
    assert_eq!(
        (o3.text.as_str(), o3.log.as_str(), o3.duration_ms),
        ("{ clippy: 101, test: 0 }", "clippy: 1 error", Some(14210.0))
    );
    let a3 = t.nodes.iter().find(|n| n.id == cm(23)).unwrap();
    let NodeBody::Assistant { blocks, .. } = &a3.body else {
        panic!("assistant")
    };
    // The calls are blocks of the code's message, right after it; Poll is dropped.
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| match b {
            AssistantBlock::ToolCall(c) => c.tool_use_id.as_str(),
            _ => "text",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "text",
            "toolu_c0de_r3",
            c3[0].tool_use_id.as_str(),
            c3[1].tool_use_id.as_str()
        ]
    );

    // Run 4: still running when the file ends.
    let r4 = code_of(&t, "toolu_c0de_r4");
    assert!(r4.call_count == 0 && r4.outcome.is_none());
    assert!(made_by(&t, "toolu_c0de_r4").is_empty());
    // A call that is not REPL has no code run, and only the calls code made name one.
    assert!(c1.iter().all(|c| c.code.is_none()));
    assert!(
        tool_calls(&t)
            .iter()
            .all(|c| c.code.is_none() || c.via_code.is_none())
    );
    // Hidden entries shown or not, the code runs are the same.
    let plain = main_t(&s, false);
    assert_eq!(code_of(&plain, "toolu_c0de_r1"), r1);
    assert_eq!(made_by(&plain, "toolu_c0de_r1"), c1);
    assert_eq!(code_of(&plain, "toolu_c0de_r3"), r3);
}

/// Code Mode code that does not wait for its Agent call ends before the call is recorded: no
/// virtual entries. The run's meta names the call and the `REPL` call it came from, so the card
/// still lists it and the run is not an orphan; a meta naming a call that is not a `REPL` stays one.
#[test]
fn code_mode_agent_call_the_code_did_not_wait_for_is_listed_under_its_code() {
    let project = fixture_dir("code_mode")
        .join("projects")
        .join(ids::CODE_MODE_DIR);
    let main = load(
        &project.join(format!("{}.jsonl", ids::CODE_MODE)),
        FileRole::Main,
        None,
    );
    let meta = |agent: &str, tool: &str, enclosing: &str| SubagentMetaFile {
        agent_id: agent.to_owned(),
        role: FileRole::Subagent,
        workflow_run_id: None,
        transcript_path: None,
        meta_json: Some(format!(
            r#"{{"agentType":"critic","description":"Critique the plan","toolUseId":"{tool}","enclosingToolUseId":"{enclosing}","requestShape":"background"}}"#
        )),
    };
    let sidecar = SessionSidecar {
        subagent_metas: vec![
            meta("acafe", "repl_c0de-agent", "toolu_c0de_r1"),
            meta("aelse", "repl_c0de-other", "toolu_c0de_missing"),
        ],
        ..SessionSidecar::default()
    };
    let s = assemble(
        ids::CODE_MODE,
        &project.join(ids::CODE_MODE),
        vec![main],
        sidecar,
    );
    let t = main_t(&s, false);
    assert_eq!(t.orphan_subagent_ids, ["aelse"]);
    let c1 = made_by(&t, "toolu_c0de_r1");
    assert_eq!(
        (c1.len(), code_of(&t, "toolu_c0de_r1").call_count),
        (5, 5),
        "the four recorded calls, then the Agent call"
    );
    let agent = *c1.last().unwrap();
    assert_eq!(
        (agent.tool_use_id.as_str(), agent.name.as_str()),
        ("repl_c0de-agent", "Agent")
    );
    assert_eq!(agent.subagent_id.as_deref(), Some("acafe"));
    assert_eq!(
        agent.input_json,
        r#"{"description":"Critique the plan","subagent_type":"critic"}"#
    );
    assert!(agent.result.as_ref().is_some_and(|r| !r.is_error));
    let run = t.subagents.iter().find(|r| r.agent_id == "acafe").unwrap();
    assert_eq!(run.tool_use_id.as_deref(), Some("repl_c0de-agent"));
    assert!(run.is_async);
    // Other code runs are as before.
    assert!(
        made_by(&t, "toolu_c0de_r3")
            .iter()
            .all(|c| c.name != "Agent")
    );
}

#[test]
fn code_mode_counts_the_calls_not_the_plumbing() {
    let s = open("code_mode", ids::CODE_MODE_DIR, ids::CODE_MODE);
    let a = summarize(&s.skeleton);
    // 8 virtual tool_uses minus the virtual REPL error marker; REPL × 4 and Poll are not counted.
    assert_eq!(a.tool_call_count, 7);
    // 2 prompts + 5 replies; the 8 virtual messages are not replies.
    assert_eq!(a.message_count, 7);
}

#[test]
fn code_mode_outputs_are_found_and_jumped_to() {
    let s = open("code_mode", ids::CODE_MODE_DIR, ids::CODE_MODE);
    let d = content::tool_detail(
        &s,
        &ToolDetailRequest {
            session_id: ids::CODE_MODE.into(),
            scope: TranscriptScope::Main,
            tool_use_id: "repl_c0de-v5".into(),
            part: ToolPart::Output,
        },
        None,
    )
    .unwrap();
    assert!(d.text.contains("port = 8080"), "{}", d.text);
    let f = find::find(
        &s,
        &FindRequest {
            session_id: ids::CODE_MODE.into(),
            scope: TranscriptScope::Main,
            branch_choices: Vec::new(),
            include_hidden: false,
            query: "port = 8080".into(),
        },
        None,
    )
    .unwrap();
    assert_eq!(f.total, 1);
    assert_eq!(f.matches[0].node_id, cm(15));
    assert_eq!(
        f.matches[0].location,
        FindLocation::ToolOutput {
            tool_use_id: "repl_c0de-v5".into()
        }
    );
    // The code's outcome is searched as the REPL call's output.
    let f = find::find(
        &s,
        &FindRequest {
            session_id: ids::CODE_MODE.into(),
            scope: TranscriptScope::Main,
            branch_choices: Vec::new(),
            include_hidden: false,
            query: "clippy: 101".into(),
        },
        None,
    )
    .unwrap();
    assert_eq!(
        f.matches.iter().map(|m| &m.location).collect::<Vec<_>>(),
        [&FindLocation::ToolOutput {
            tool_use_id: "toolu_c0de_r3".into()
        }]
    );
    // A search hit on a virtual entry lands on its REPL call's node.
    let j = transcript::assemble::jump::jump(
        &s,
        &transcript::model::JumpRequest {
            session_id: ids::CODE_MODE.into(),
            node_id: cm(20),
            agent_id: None,
            tool_use_id: None,
        },
    )
    .unwrap();
    assert_eq!(
        (j.node_id.as_str(), j.tool_use_id.as_deref()),
        (cm(15).as_str(), Some("repl_c0de-v5"))
    );
    assert!(!j.needs_hidden && j.branch_choices.is_empty());
    let j = transcript::assemble::jump::jump(
        &s,
        &transcript::model::JumpRequest {
            session_id: ids::CODE_MODE.into(),
            node_id: cm(19),
            agent_id: None,
            tool_use_id: None,
        },
    )
    .unwrap();
    assert_eq!(
        (j.node_id.as_str(), j.tool_use_id.as_deref()),
        (cm(15).as_str(), Some("repl_c0de-v5"))
    );
}

/// The `REPL` result can hang off the call instead of lying on the path: the `Poll` made with it and
/// its result (carrying the outcome) are on the path, the virtual entries after them (claude 2.1.327,
/// `tools.AskUserQuestion`). The outcome is still the code's.
#[test]
fn code_mode_outcome_found_when_the_repl_result_is_off_the_path() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"ask me"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"r1","name":"REPL","input":{"code":"await tools.AskUserQuestion({})"}}]}}"#,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"p1","name":"Poll","input":{}}]}}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"(dispatched #48)"}]},"toolUseResult":{"evalId":48,"asyncDispatched":true}}"#,
        r#"{"type":"user","uuid":"u3","parentUuid":"a2","timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"p1","content":"<event kind=\"repl-eval\" eval=\"48\" ms=\"97120\" status=\"settled\">\n<result>{ answers: {} }</result>\n</event>"}]}}"#,
        r#"{"type":"assistant","uuid":"v1","parentUuid":"u3","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"id":"mv","model":"<synthetic>","content":[{"type":"tool_use","id":"repl_q","name":"AskUserQuestion","input":{"questions":[]}}]}}"#,
        r#"{"type":"user","uuid":"v2","parentUuid":"v1","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"repl_q","content":""}]},"toolUseResult":{"questions":[],"answers":{}}}"#,
        r#"{"type":"assistant","uuid":"a3","parentUuid":"v2","timestamp":"2026-01-01T00:01:05Z","message":{"id":"m2","model":"x","content":[{"type":"text","text":"thanks"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let run = code_of(&t, "r1");
    let made = made_by(&t, "r1");
    assert_eq!((made.len(), run.call_count), (1, 1));
    assert_eq!(made[0].name, "AskUserQuestion");
    let o = run.outcome.as_ref().expect("outcome");
    assert_eq!(
        (o.text.as_str(), o.duration_ms),
        ("{ answers: {} }", Some(97120.0))
    );
}

/// A file the code read as a picture: the virtual Read's own result has no picture (`toolUseResult`
/// `{"type":"image"}`, the data dropped); the pictures were shown to Claude with the code's outcome,
/// in the `Poll` result. They go to the Reads in order, and are fetched from where they are stored.
#[test]
fn code_mode_pictures_the_code_read_are_shown_on_their_reads() {
    let img = |data: &str| {
        format!(
            r#"{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{data}"}}}}"#
        )
    };
    let poll = format!(
        r#"{{"type":"user","uuid":"u3","parentUuid":"a2","timestamp":"2026-01-01T00:01:00Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"p1","content":[{{"type":"text","text":"<event kind=\"repl-eval\" eval=\"7\" ms=\"120\" status=\"settled\">\n<result>ok</result>\n</event>"}},{},{}]}}]}},"toolUseResult":{{"eventCount":1,"media":[[{},{}]]}}}}"#,
        img("QUFB"),
        img("QkJC"),
        img("QUFB"),
        img("QkJC")
    );
    let read = |n: u32, parent: &str, file: &str| {
        [
            format!(
                r#"{{"type":"assistant","uuid":"v{n}a","parentUuid":"{parent}","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{{"id":"mv{n}","model":"<synthetic>","content":[{{"type":"tool_use","id":"repl_{n}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
            ),
            format!(
                r#"{{"type":"user","uuid":"v{n}b","parentUuid":"v{n}a","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"repl_{n}","content":""}}]}},"toolUseResult":{{"type":"image","file":{{"base64":"","type":"image/png","originalSize":3}}}}}}"#
            ),
        ]
    };
    let [r1a, r1b] = read(1, "u3", "/tmp/a.png");
    let [r3a, r3b] = read(3, "v2b", "/tmp/b.png");
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"look"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"r1","name":"REPL","input":{"code":"await tools.Read({})"}}]}}"#,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"p1","name":"Poll","input":{}}]}}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"(dispatched #7)"}]},"toolUseResult":{"evalId":7,"asyncDispatched":true}}"#,
        &poll,
        &r1a,
        &r1b,
        r#"{"type":"assistant","uuid":"v2a","parentUuid":"v1b","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"id":"mv2","model":"<synthetic>","content":[{"type":"tool_use","id":"repl_2","name":"Read","input":{"file_path":"/tmp/notes.txt"}}]}}"#,
        r#"{"type":"user","uuid":"v2b","parentUuid":"v2a","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"repl_2","content":""}]},"toolUseResult":{"type":"text","file":{"content":"an image\n","startLine":1}}}"#,
        &r3a,
        &r3b,
        r#"{"type":"assistant","uuid":"a3","parentUuid":"v3b","timestamp":"2026-01-01T00:01:05Z","message":{"id":"m2","model":"x","content":[{"type":"text","text":"seen"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let made = made_by(&t, "r1");
    let pictures = |k: usize| &made[k].result.as_ref().expect("result").images;
    assert_eq!(made.len(), 3);
    assert_eq!(
        [pictures(0).len(), pictures(1).len(), pictures(2).len()],
        [1, 0, 1]
    );
    let data = |k: usize| {
        content::image(
            &s,
            &ImageRequest {
                session_id: "s".into(),
                scope: TranscriptScope::Main,
                image: pictures(k)[0].clone(),
            },
            None,
        )
        .expect("image data")
    };
    assert_eq!(pictures(0)[0].bytes, 3.0);
    assert_eq!(
        (data(0).media_type.as_str(), data(0).data_base64.as_str()),
        ("image/png", "QUFB")
    );
    assert_eq!(data(2).data_base64, "QkJC");
}

/// The same when the outcome arrives in a `poll_events` attachment: the picture is in its `media`.
#[test]
fn code_mode_pictures_delivered_in_an_attachment() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"look"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"r1","name":"REPL","input":{"code":"await tools.Read({})"}}]}}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"(dispatched #7)"}]},"toolUseResult":{"evalId":7,"asyncDispatched":true}}"#,
        r#"{"type":"attachment","uuid":"at1","parentUuid":"u2","timestamp":"2026-01-01T00:01:00Z","attachment":{"type":"poll_events","envelopes":["<event kind=\"task\">x</event>","<event kind=\"repl-eval\" eval=\"7\" ms=\"5\"><result>ok</result></event>"],"kinds":["task","repl-eval"],"media":[[],[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"Q0ND"}}]]}}"#,
        r#"{"type":"assistant","uuid":"v1a","parentUuid":"at1","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"id":"mv1","model":"<synthetic>","content":[{"type":"tool_use","id":"repl_1","name":"Read","input":{"file_path":"/tmp/a.jpg"}}]}}"#,
        r#"{"type":"user","uuid":"v1b","parentUuid":"v1a","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"repl_1","content":""}]},"toolUseResult":{"type":"image","file":{"base64":"","type":"image/jpeg"}}}"#,
        r#"{"type":"assistant","uuid":"a3","parentUuid":"v1b","timestamp":"2026-01-01T00:01:05Z","message":{"id":"m2","model":"x","content":[{"type":"text","text":"seen"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let made = made_by(&t, "r1");
    let image = made[0].result.as_ref().expect("result").images[0].clone();
    let data = content::image(
        &s,
        &ImageRequest {
            session_id: "s".into(),
            scope: TranscriptScope::Main,
            image,
        },
        None,
    )
    .expect("image data");
    assert_eq!(
        (data.media_type.as_str(), data.data_base64.as_str()),
        ("image/jpeg", "Q0ND")
    );
}

#[test]
fn code_mode_call_that_failed_itself_has_its_error_as_outcome() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"go"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"r1","name":"REPL","input":{"code":"1"}}]}}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","is_error":true,"content":"<tool_use_error>InputValidationError</tool_use_error>"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let o = code_of(&t, "r1").outcome.as_ref().expect("outcome");
    assert!(o.is_error);
    assert_eq!(
        o.text,
        "<tool_use_error>InputValidationError</tool_use_error>"
    );
    assert_eq!(o.duration_ms, None);
}

/// When virtual entries cannot be attributed to a `REPL` call (for instance if the code tool is
/// renamed), their tool calls and outputs still appear as nodes in the assembled transcript instead
/// of being dropped.
#[test]
fn code_mode_unowned_virtual_calls_still_appear_in_transcript() {
    let f = from_lines(&[
        r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"check config"}}"#,
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"r1","name":"Script","input":{"code":"await tools.Read({})"}}]}}"#,
        r#"{"type":"assistant","uuid":"a2","parentUuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"p1","name":"Poll","input":{}}]}}"#,
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"(dispatched #48)"}]},"toolUseResult":{"evalId":48,"asyncDispatched":true}}"#,
        r#"{"type":"user","uuid":"u3","parentUuid":"a2","timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"p1","content":"<event kind=\"repl-eval\" eval=\"48\" ms=\"120\" status=\"settled\">\n<result>ok</result>\n</event>"}]}}"#,
        r#"{"type":"assistant","uuid":"v1","parentUuid":"u3","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"id":"mv","model":"<synthetic>","content":[{"type":"tool_use","id":"repl_r","name":"Read","input":{"file_path":"/tmp/default.toml"}}]}}"#,
        r#"{"type":"user","uuid":"v2","parentUuid":"v1","isVirtual":true,"timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"repl_r","content":""}]},"toolUseResult":{"type":"text","file":{"content":"port = 8080\n","startLine":1}}}"#,
        r#"{"type":"assistant","uuid":"a3","parentUuid":"v2","timestamp":"2026-01-01T00:01:05Z","message":{"id":"m2","model":"x","content":[{"type":"text","text":"done"}]}}"#,
    ]);
    let s = assemble(
        "s",
        Path::new("/nonexistent"),
        vec![f],
        SessionSidecar::default(),
    );
    let t = main_t(&s, false);
    let read = call(&t, "repl_r");
    assert_eq!(read.name, "Read");
    assert_eq!(read.via_code, None);
    let out = read.result.as_ref().expect("virtual tool result");
    assert!(out.text.contains("port = 8080"), "{}", out.text);
}
