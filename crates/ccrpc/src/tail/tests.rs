use serde_json::json;

use super::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcripts")
        .join(name)
}

fn blocks(evs: &[TxEvent]) -> Vec<&Block> {
    evs.iter()
        .filter_map(|e| {
            if let TxEvent::Block(b) = e {
                Some(b.as_ref())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn reads_prompts_blocks_turn_ends_and_steer() {
    let mut t = Tailer::new(&fixture("steer_ask_permission.jsonl"));
    let evs = t.poll();
    let prompts: Vec<_> = evs
        .iter()
        .filter_map(|e| {
            if let TxEvent::UserPrompt { text, .. } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        prompts[0],
        "Reply with exactly the word PONG and nothing else."
    );
    let turn_ends = evs
        .iter()
        .filter(|e| matches!(e, TxEvent::TurnEnd { .. }))
        .count();
    assert!(turn_ends >= 5, "{turn_ends}");
    let bs = blocks(&evs);
    assert!(
        bs.iter()
            .any(|b| b.kind == BlockKind::Text && b.text.as_deref() == Some("PONG"))
    );
    let ask = bs
        .iter()
        .find(|b| b.name.as_deref() == Some("AskUserQuestion"))
        .unwrap();
    assert!(ask.input.as_ref().unwrap()["questions"].is_array());
    assert!(bs.iter().any(|b| {
        b.kind == BlockKind::ToolResult
            && b.content
                .as_deref()
                .unwrap_or("")
                .starts_with("Your questions have been answered")
    }));
    // steer: enqueue marker and the queued_command delivered as a queued user block
    assert!(
        evs.iter().any(
            |e| matches!(e, TxEvent::Enqueued { content, .. } if content.starts_with("STEER:"))
        )
    );
    assert!(
        bs.iter()
            .any(|b| b.queued && b.text.as_deref().unwrap_or("").starts_with("STEER:"))
    );
    // every tool_use got its result, so nothing is pending at the end
    assert!(t.pending().is_empty(), "{:?}", t.pending());
    assert!(t.last_text().is_some());
}

#[test]
fn subagent_blocks_are_tagged_and_command_records_skipped() {
    let mut t = Tailer::new(&fixture("subagent_deny.jsonl"));
    let evs = t.poll();
    let bs = blocks(&evs);
    assert!(
        !bs.iter()
            .any(|b| b.text.as_deref().unwrap_or("").contains("<command-name>"))
    );
    let sub: Vec<_> = bs.iter().filter(|b| b.agent_id.is_some()).collect();
    assert!(!sub.is_empty());
    assert!(
        sub.iter()
            .all(|b| b.agent_id.as_deref() == Some("a1f4e2d8aa4a5bb8d")
                && b.parent_tool_use_id.as_deref() == Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243"))
    );
    assert!(sub.iter().any(|b| b.name.as_deref() == Some("Bash")));
    let agent_call = bs
        .iter()
        .find(|b| b.name.as_deref() == Some("Agent"))
        .unwrap();
    assert_eq!(
        agent_call.id.as_deref(),
        Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243")
    );
    // the denied rm still has an (error) tool_result, so nothing is left pending
    assert!(t.pending().is_empty(), "{:?}", t.pending());
}

#[test]
fn incremental_reads_and_pending_tools() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let src = std::fs::read_to_string(fixture("steer_ask_permission.jsonl")).unwrap();
    let lines: Vec<&str> = src.lines().collect();
    let cut = lines
        .iter()
        .position(|l| l.contains("\"name\": \"AskUserQuestion\""))
        .unwrap();
    // write up to and including the AskUserQuestion tool_use, plus half of the next line
    let mut head = lines[..=cut].join("\n");
    head.push('\n');
    head.push_str(&lines[cut + 1][..10]);
    std::fs::write(&main, &head).unwrap();
    let mut t = Tailer::new(&main);
    assert!(!t.poll().is_empty(), "first poll returns the history");
    assert_eq!(
        t.pending_named("AskUserQuestion").map(|p| p.name.as_str()),
        Some("AskUserQuestion")
    );
    assert!(
        t.poll().is_empty(),
        "catch-up is silent and the partial line waits"
    );
    let mut rest = lines[cut + 1..].join("\n");
    rest.push('\n');
    std::fs::write(
        &main,
        format!("{}{}", lines[..=cut].join("\n") + "\n", rest),
    )
    .unwrap();
    let evs = t.poll();
    assert!(!evs.is_empty());
    assert!(t.pending_named("AskUserQuestion").is_none());
}

#[test]
fn model_and_effort_outputs_say_what_was_set() {
    assert_eq!(
        local_setting(
            "<local-command-stdout>Set model to `Sonnet 4.5` and saved as your default for new sessions\n         ANTHROPIC_MODEL is set</local-command-stdout>"
        ),
        Some((Some("Sonnet 4.5".into()), None))
    );
    assert_eq!(
        local_setting("Set model to Opus 4.7 (default) and saved as your default for new sessions"),
        Some((Some("Opus 4.7".into()), None))
    );
    assert_eq!(
        local_setting("Kept model as `Opus 4.7`"),
        Some((Some("Opus 4.7".into()), None))
    );
    assert_eq!(
        local_setting(
            "<local-command-stdout>Set effort level to low (saved as your default for new sessions): Quick</local-command-stdout>"
        ),
        Some((None, Some("low".into())))
    );
    assert_eq!(
        local_setting("<local-command-stdout>Cancelled</local-command-stdout>"),
        None
    );
    // Both record shapes: a `user` entry (2.1.316) and a `system` / `local_command` one.
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let user = json!({"type": "user", "uuid": "o1", "message": {"role": "user",
            "content": "<local-command-stdout>Set effort level to low (saved as your default for new sessions): Quick</local-command-stdout>"}});
    let system = json!({"type": "system", "subtype": "local_command", "uuid": "o2",
            "content": "<local-command-stdout>Set model to `Fable 5.1` and saved as your default for new sessions</local-command-stdout>"});
    let reply = json!({"type": "assistant", "uuid": "a1", "effort": "high",
            "message": {"id": "m1", "model": "claude-fable-5-1", "role": "assistant",
                "content": [{"type": "text", "text": "ok"}], "usage": {"input_tokens": 1, "output_tokens": 1}}});
    write_lines(&main, &[user, system, reply]);
    let evs = Tailer::new(&main).poll();
    let got: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            TxEvent::Setting { model, effort } => Some((model.clone(), effort.clone())),
            TxEvent::Usage { model, effort, .. } => Some((model.clone(), effort.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (None, Some("low".into())),
            (Some("Fable 5.1".into()), None),
            (Some("claude-fable-5-1".into()), Some("high".into())),
        ]
    );
}

#[test]
fn a_bang_command_s_output_follows_its_record() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let input = json!({"type": "user", "uuid": "b1", "message": {"role": "user",
            "content": "<bash-input> echo hi</bash-input>"}});
    let output = json!({"type": "user", "uuid": "b2", "parentUuid": "b1", "message": {"role": "user",
            "content": "<bash-stdout>hi</bash-stdout><bash-stderr></bash-stderr>"}});
    write_lines(&main, &[input, output]);
    let evs = Tailer::new(&main).poll();
    assert!(
        !evs.iter().any(|e| matches!(e, TxEvent::Block(_))),
        "{evs:?}"
    );
    let outs: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            TxEvent::BashOutput {
                parent_uuid,
                stdout,
                stderr,
            } => Some((parent_uuid.clone(), stdout.clone(), stderr.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(outs, vec![(Some("b1".into()), "hi".into(), String::new())]);
}

#[test]
fn slash_command_records_confirm_as_typed_without_blocks() {
    // Claude Code 2.1.318: a local command as a `system` entry, a prompt-type one as a `user` entry.
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let local = json!({"type": "system", "subtype": "local_command", "uuid": "c1",
            "content": "<command-name>/rename</command-name>\n            <command-message>rename</command-message>\n            <command-args>probe-a</command-args>"});
    let output = json!({"type": "system", "subtype": "local_command", "uuid": "c2",
            "content": "<local-command-stdout>Session renamed to: probe-a</local-command-stdout>"});
    let skill = json!({"type": "user", "uuid": "c3", "message": {"role": "user",
            "content": "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args></command-args>"}});
    write_lines(&main, &[local, output, skill]);
    let evs = Tailer::new(&main).poll();
    let prompts: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            TxEvent::UserPrompt { uuid, text, .. } => Some((uuid.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, vec![("c1", "/rename probe-a"), ("c3", "/review")]);
    assert!(blocks(&evs).is_empty());
}

#[test]
fn queue_follows_the_queue_operations() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let op = |op: &str, content: Option<&str>| {
        let mut v = json!({"type": "queue-operation", "operation": op});
        if let Some(c) = content {
            v["content"] = json!(c);
        }
        v
    };
    write_lines(
        &main,
        &[
            op("enqueue", Some("a")),
            op("enqueue", Some("b")),
            op("enqueue", Some("c")),
        ],
    );
    let mut t = Tailer::new(&main);
    let queues = |evs: Vec<TxEvent>| -> Vec<Vec<String>> {
        evs.into_iter()
            .filter_map(|e| {
                if let TxEvent::Queue(q) = e {
                    Some(q.into_iter().map(|i| i.text).collect())
                } else {
                    None
                }
            })
            .collect()
    };
    assert_eq!(queues(t.poll()).last().unwrap(), &vec!["a", "b", "c"]);
    let more = [
        op("remove", Some("b")),
        op("dequeue", None),
        op("popAll", None),
    ];
    let text: String = more.iter().map(|v| format!("{v}\n")).collect();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&main)
        .unwrap();
    std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
    let q = queues(t.poll());
    assert_eq!(q, vec![vec!["a", "c"], vec!["c"], vec![]]);
    assert!(t.queue().is_empty());
    // A relayed message is enqueued with a `hop-chain` attribute its `remove` lacks.
    let relayed = |attrs: &str| {
        format!(
            "<cross-session-message from=\"uds:/tmp/a.sock\"{attrs} from-name=\"c6\">\nhello\n</cross-session-message>"
        )
    };
    let more = [
        op("enqueue", Some(&relayed(" hop-chain=\"8a7e\""))),
        op("remove", Some(&relayed(""))),
    ];
    let text: String = more.iter().map(|v| format!("{v}\n")).collect();
    std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
    assert_eq!(queues(t.poll()).last().unwrap(), &Vec::<String>::new());
    // `↑` records each item it takes back; a message from another Session stays queued.
    let more = [
        op("enqueue", Some("d")),
        op("enqueue", Some(&relayed(""))),
        op("enqueue", Some("e")),
        op("popAll", Some("d")),
        op("popAll", Some("e")),
    ];
    let text: String = more.iter().map(|v| format!("{v}\n")).collect();
    std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
    let evs = t.poll();
    let popped: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            TxEvent::Popped { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(popped, ["d", "e"]);
    assert_eq!(queue_texts(&t), [relayed("")]);
}

/// EnterWorktree moves the transcript with its Session directory: what was read stays read
/// (pending tools too), only what is written after the move is new, and the Subagent Run
/// already being followed is read at its new place.
#[test]
fn a_moved_transcript_goes_on_where_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&from).unwrap();
    let main = from.join("s.jsonl");
    let tool_use = json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
            "message": {"id": "m-a1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_grant", "name": "mcp__x__y", "input": {}}]}});
    write_lines(&main, &[prompt("u1", None, "before the move", 1), tool_use]);
    let agents = subagents_dir(&main);
    std::fs::create_dir_all(&agents).unwrap();
    write_lines(
        &agents.join("agent-ag1.jsonl"),
        &[prompt("x1", None, "sub prompt", 1)],
    );
    let mut t = Tailer::new(&main);
    assert!(!blocks(&t.poll()).is_empty());

    std::fs::rename(&from, &to).unwrap();
    // Polled before anything says where it went: nothing is lost.
    assert!(t.poll().is_empty());
    assert_eq!(t.pending().len(), 1);

    let moved = to.join("s.jsonl");
    t.moved_to(&moved);
    let mut lines = std::fs::read_to_string(&moved).unwrap();
    lines.push_str(&format!("{}\n", reply("r1", "a1", "after the move", 2)));
    std::fs::write(&moved, lines).unwrap();
    let mut sub = std::fs::read_to_string(subagents_dir(&moved).join("agent-ag1.jsonl")).unwrap();
    sub.push_str(&format!("{}\n", reply("x2", "x1", "sub after", 2)));
    std::fs::write(subagents_dir(&moved).join("agent-ag1.jsonl"), sub).unwrap();

    let after = t.poll();
    let texts: Vec<&str> = blocks(&after)
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect();
    assert_eq!(texts, ["after the move", "sub after"], "{after:?}");
    assert_eq!(t.main_path(), moved);
    assert_eq!(t.pending().len(), 1);
}

/// A Poll takes the harness event it waits for, not the oldest queued one, and the older one is
/// `remove`d after: what was delivered shows which one left, so the queue ends empty rather
/// than keeping the newer one for good. Seen in real Sessions: the Poll result written after
/// an unrelated tool result (ford 14:01:20); written only after the next `enqueue` and the
/// `remove`, and in a Subagent Run's file, whose code is handed its events through the main
/// queue (15:12:45). A dequeued prompt is the oldest, as before.
#[test]
fn a_dequeue_is_settled_by_what_was_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let op = |op: &str, content: Option<&str>| {
        let mut v = json!({"type": "queue-operation", "operation": op});
        if let Some(c) = content {
            v["content"] = json!(c);
        }
        v
    };
    let event = |nonce: &str, eval: u32| {
        format!(
            "<event nonce=\"{nonce}\" kind=\"repl-eval\" eval=\"{eval}\" status=\"settled\">\n<result>ok {eval}</result>\n</event>"
        )
    };
    let result = |uuid: &str, text: &str| {
        json!({"type": "user", "uuid": uuid, "parentUuid": "a0",
                "message": {"role": "user", "content": [{"type": "tool_result",
                    "tool_use_id": format!("toolu_{uuid}"), "content": text}]}})
    };
    let poll = |uuid: &str, nonce: &str, ev: &str| {
        result(
            uuid,
            &format!("<system>authentic event nonces for this delivery: {nonce}</system>\n{ev}"),
        )
    };
    let append = |path: &Path, v: &Value| {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut f, format!("{v}\n").as_bytes()).unwrap();
    };
    let (older, newer, next) = (
        event("dl4j1wAU", 55),
        event("QQNj8dxd", 56),
        event("Z3Pd3XQg", 57),
    );

    // The Poll result comes after an unrelated tool result, before the `remove`.
    let main = dir.path().join("a.jsonl");
    write_lines(
        &main,
        &[
            op("enqueue", Some(&older)),
            op("enqueue", Some(&newer)),
            op("dequeue", None),
            result("u1", "(dispatched #57)"),
            poll("u2", "QQNj8dxd", &newer),
        ],
    );
    let mut t = Tailer::new(&main);
    t.poll();
    assert_eq!(queue_texts(&t), std::slice::from_ref(&older));
    // The guess put back keeps its number.
    assert_eq!(t.queue()[0].id, 1);
    append(&main, &op("remove", Some(&older)));
    t.poll();
    assert!(t.queue().is_empty(), "{:?}", t.queue());

    // The `remove` and the next `enqueue` come first; the Poll result is a Subagent Run's.
    let main = dir.path().join("b.jsonl");
    write_lines(
        &main,
        &[
            op("enqueue", Some(&older)),
            op("enqueue", Some(&newer)),
            op("dequeue", None),
            op("enqueue", Some(&next)),
            op("remove", Some(&older)),
        ],
    );
    let agents = subagents_dir(&main);
    std::fs::create_dir_all(&agents).unwrap();
    let sub = agents.join("agent-ag1.jsonl");
    write_lines(&sub, &[prompt("x1", None, "sub prompt", 1)]);
    let mut t = Tailer::new(&main);
    t.poll();
    assert_eq!(queue_texts(&t), [newer.clone(), next.clone()]);
    assert_eq!(t.queue().iter().map(|q| q.id).collect::<Vec<_>>(), [2, 3]);
    append(&sub, &poll("x2", "QQNj8dxd", &newer));
    t.poll();
    assert_eq!(queue_texts(&t), std::slice::from_ref(&next));

    let main = dir.path().join("p.jsonl");
    write_lines(
        &main,
        &[
            op("enqueue", Some("first")),
            op("enqueue", Some("second")),
            op("dequeue", None),
            prompt("u1", None, "first", 1),
        ],
    );
    let mut t = Tailer::new(&main);
    t.poll();
    assert_eq!(queue_texts(&t), ["second"]);
}

/// A main file that shrank was replaced: nothing read from the old one is kept (queue,
/// pending tools, entry offsets), as if tailing the new file from the start.
#[test]
fn a_replaced_main_file_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let enqueue =
        |c: &str| json!({"type": "queue-operation", "operation": "enqueue", "content": c});
    let tool_use = json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
            "message": {"id": "m-a1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_old", "name": "Bash", "input": {"command": "ls"}}]}});
    write_lines(
        &main,
        &[
            prompt("u1", None, "a long first prompt from the old file", 1),
            tool_use,
            enqueue("old one"),
            enqueue("old two"),
        ],
    );
    // A Subagent Run file next to it, and a reply whose usage the new file has again.
    let agents = subagents_dir(&main);
    std::fs::create_dir_all(&agents).unwrap();
    write_lines(
        &agents.join("agent-ag1.jsonl"),
        &[prompt("x1", None, "sub prompt", 1)],
    );
    let mut billed = reply("r1", "u1", "done", 2);
    billed["message"]["usage"] = json!({"input_tokens": 10, "output_tokens": 3});
    let mut old_lines = std::fs::read_to_string(&main).unwrap();
    old_lines.push_str(&format!("{billed}\n"));
    std::fs::write(&main, old_lines).unwrap();
    let mut t = Tailer::new(&main);
    t.keep_offsets();
    let first = t.poll();
    assert!(
        blocks(&first)
            .iter()
            .any(|b| b.agent_id.as_deref() == Some("ag1"))
    );
    assert!(first.iter().any(|e| matches!(e, TxEvent::Usage { .. })));
    assert_eq!(queue_texts(&t), ["old one", "old two"]);
    assert!(t.pending_by_id("toolu_old").is_some());
    write_lines(
        &main,
        &[prompt("n1", None, "new", 1), enqueue("new"), billed],
    );
    let evs = t.poll();
    // Only the main file starts over: the Subagent Run and the usage are not reported again.
    assert!(
        !blocks(&evs).iter().any(|b| b.agent_id.is_some()),
        "{evs:?}"
    );
    assert!(
        !evs.iter().any(|e| matches!(e, TxEvent::Usage { .. })),
        "{evs:?}"
    );
    assert!(blocks(&evs).iter().any(|b| b.uuid == "n1"));
    assert_eq!(queue_texts(&t), ["new"]);
    // Numbers given to the old file's items are not given again.
    assert_eq!(t.queue()[0].id, 3);
    assert!(t.pending().is_empty());
    assert_eq!(t.offset_of("u1"), None);
    assert_eq!(t.offset_of("n1"), Some(0));
    assert_eq!(t.read_to(), std::fs::metadata(&main).unwrap().len());
}

#[test]
fn interruption_note_is_a_block_but_not_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let line = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"[Request interrupted by user]"}}"#;
    std::fs::write(&main, format!("{line}\n")).unwrap();
    let evs = Tailer::new(&main).poll();
    assert!(!evs.iter().any(|e| matches!(e, TxEvent::UserPrompt { .. })));
    assert_eq!(blocks(&evs).len(), 1);
}

#[test]
fn wrapped_paste_with_an_image_reads_as_sent() {
    // A long paste after an image, as a real account records it (2.1.317).
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let text = "[Image #1]\n\n<pasted_content id=\"1b44\">\nholder_id: m1\n\n现在有问题\n</pasted_content id=\"1b44\">\n";
    let line = json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": [
        {"type": "text", "text": text},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
    ]}});
    let note = json!({"type": "user", "uuid": "u2", "isMeta": true, "message": {"role": "user", "content": [
        {"type": "text", "text": "[Image: source: /tmp/x.png]"},
    ]}});
    write_lines(&main, &[line, note]);
    let t = Tailer::new(&main);
    let evs = {
        let mut t = Tailer::new(&main);
        t.poll()
    };
    // The image note is not a block; the prompt's block names its image, fetched by uuid.
    assert_eq!(blocks(&evs).len(), 1, "{evs:?}");
    assert_eq!(blocks(&evs)[0].images, vec!["image/png".to_string()]);
    assert_eq!(
        t.image("u1", 0),
        Some(("image/png".into(), "iVBORw0KGgo=".into()))
    );
    assert_eq!(t.image("u1", 1), None);
    assert_eq!(t.image("u2", 0), None);
    let prompt = evs.iter().find_map(|e| match e {
        TxEvent::UserPrompt { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(
        prompt.as_deref().map(str::trim),
        Some("[Image #1]\n\nholder_id: m1\n\n现在有问题")
    );
    assert_eq!(
        blocks(&evs)[0].text.as_deref().map(str::trim),
        Some("[Image #1]\n\nholder_id: m1\n\n现在有问题")
    );
}

#[test]
fn main_blocks_reads_whole_file() {
    let path = fixture("steer_ask_permission.jsonl");
    let (bs, cursor) = main_blocks(&path, 0);
    // (The fixture is trimmed: its chain is cut in places, so only counts are checked.)
    assert!(bs.len() > 20);
    assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
    assert!(main_blocks(&path, cursor).0.is_empty());
}

/// Writes `lines` (one JSON object each) to `path`.
fn write_lines(path: &Path, lines: &[Value]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, text).unwrap();
}

fn prompt(uuid: &str, parent: Option<&str>, text: &str, ts: u32) -> Value {
    json!({"type": "user", "uuid": uuid, "parentUuid": parent,
            "timestamp": format!("2026-11-03T00:00:{ts:02}Z"),
            "message": {"role": "user", "content": text}})
}

fn reply(uuid: &str, parent: &str, text: &str, ts: u32) -> Value {
    json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
            "timestamp": format!("2026-11-03T00:00:{ts:02}Z"),
            "message": {"id": format!("m-{uuid}"), "role": "assistant",
                "content": [{"type": "text", "text": text}]}})
}

fn last_prompt(leaf: &str) -> Value {
    json!({"type": "last-prompt", "leafUuid": leaf})
}

fn attachment(uuid: &str, parent: &str) -> Value {
    json!({"type": "attachment", "uuid": uuid, "parentUuid": parent,
            "attachment": {"type": "todo_reminder"}})
}

fn interruption(uuid: &str, parent: &str) -> Value {
    json!({"type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": "[Request interrupted by user]"}})
}

/// Tails `lines`, then `more` appended; returns the events of the second poll.
fn tail_then(main: &Path, lines: &[Value], more: &[Value]) -> (Tailer, Vec<TxEvent>) {
    write_lines(main, lines);
    let mut t = Tailer::new(main);
    assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
    let all: Vec<Value> = lines.iter().chain(more).cloned().collect();
    write_lines(main, &all);
    let evs = t.poll();
    (t, evs)
}

fn kinds(evs: &[TxEvent]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            TxEvent::Branch { at_uuid } => {
                Some(format!("branch:{}", at_uuid.as_deref().unwrap_or("-")))
            }
            TxEvent::Block(b) => Some(format!("block:{}", b.uuid)),
            _ => None,
        })
        .collect()
}

fn history() -> Vec<Value> {
    vec![
        prompt("u1", None, "first", 1),
        reply("a1", "u1", "one", 2),
        last_prompt("a1"),
        prompt("u2", Some("a1"), "second", 3),
        reply("a2", "u2", "two", 4),
        last_prompt("a2"),
    ]
}

/// Moved to the background: the new transcript starts with a copy of the old one, written
/// after ccrpc switched to it (and possibly in several appends). The copy gives no blocks,
/// usage, turn ends or prompts; what follows it does.
#[test]
fn copied_entries_of_a_continued_session_produce_no_events() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = (dir.path().join("old.jsonl"), dir.path().join("new.jsonl"));
    let usage = json!({"input_tokens": 10, "output_tokens": 5});
    let replied = |uuid: &str, parent: &str, id: &str| {
        json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
                "message": {"id": id, "model": "claude-sonnet-4-5", "role": "assistant", "usage": usage,
                    "content": [{"type": "text", "text": "old"}]}})
    };
    let copy = vec![
        prompt("u1", None, "first", 1),
        replied("a1", "u1", "m1"),
        json!({"type": "system", "subtype": "turn_duration", "uuid": "d1", "parentUuid": "a1", "durationMs": 10}),
        interruption("i1", "d1"),
        json!({"type": "cost-state", "totalCostUSD": 1.0}),
    ];
    write_lines(&old, &copy);
    let mut t = Tailer::new(&new);
    t.copied_from(entry_uuids(&old));
    assert!(t.poll().is_empty(), "no file yet");
    // The copy arrives in two appends, a line without uuid among it.
    write_lines(&new, &copy[..2]);
    assert!(t.poll().is_empty());
    write_lines(&new, &copy);
    assert!(t.poll().is_empty());
    let mut all = copy.clone();
    all.extend([
            prompt("u2", Some("i1"), "second", 5),
            replied("a2", "u2", "m2"),
            json!({"type": "system", "subtype": "turn_duration", "uuid": "d2", "parentUuid": "a2", "durationMs": 20}),
        ]);
    write_lines(&new, &all);
    let evs = t.poll();
    assert_eq!(kinds(&evs), ["block:u2", "block:a2"]);
    let usages = evs
        .iter()
        .filter(|e| matches!(e, TxEvent::Usage { .. }))
        .count();
    assert_eq!(usages, 1);
    assert!(evs.iter().any(|e| matches!(
        e,
        TxEvent::TurnEnd {
            duration_ms: Some(20)
        }
    )));
    assert!(
        evs.iter()
            .any(|e| matches!(e, TxEvent::UserPrompt { text, .. } if text == "second"))
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, TxEvent::UserPrompt { text, .. } if text == "first"))
    );
}

#[test]
fn rewind_reports_the_branch_before_its_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    // Rewind to after "one" and ask something else; the leaf marker comes later.
    let (_, evs) = tail_then(
        &main,
        &history(),
        &[
            prompt("u3", Some("a1"), "other", 5),
            reply("a3", "u3", "three", 6),
        ],
    );
    assert_eq!(kinds(&evs), ["branch:a1", "block:u3", "block:a3"]);
    let texts: Vec<_> = main_blocks(&main, 0)
        .0
        .into_iter()
        .filter_map(|b| b.text)
        .collect();
    assert_eq!(texts, ["first", "one", "other", "three"]);
}

#[test]
fn a_branch_starting_with_an_attachment_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let (_, evs) = tail_then(
        &main,
        &history(),
        &[attachment("x3", "a1"), prompt("u3", Some("x3"), "other", 5)],
    );
    assert_eq!(kinds(&evs)[0], "branch:a1");
}

#[test]
fn rewinding_to_the_start_is_a_branch_too() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let (_, evs) = tail_then(&main, &history(), &[prompt("u3", None, "fresh", 5)]);
    assert_eq!(kinds(&evs)[0], "branch:-");
    let texts: Vec<_> = main_blocks(&main, 0)
        .0
        .into_iter()
        .filter_map(|b| b.text)
        .collect();
    assert_eq!(texts, ["fresh"]);
}

fn system(uuid: &str, parent: Option<&str>, subtype: &str, logical: Option<&str>) -> Value {
    json!({"type": "system", "subtype": subtype, "uuid": uuid, "parentUuid": parent,
            "logicalParentUuid": logical, "content": ""})
}

#[test]
fn narration_is_a_text_block_and_the_last_text() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let think = |uuid: &str, parent: &str, text: &str, sig: &str| {
        json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "message": {"id": uuid, "role": "assistant",
                "content": [{"type": "thinking", "thinking": text, "signature": sig}]}})
    };
    write_lines(
        &main,
        &[
            prompt("u1", None, "hi", 1),
            think(
                "a1",
                "u1",
                "private",
                "CAQSLgoRCBMYAjgBQgh0aGlua2luZxIMAAECAwQFBgcICQoLGgwMDQ4PEBESExQVFhc=",
            ),
            think(
                "a2",
                "a1",
                "Checking the layout.",
                "CAQSLwoRCBMYAjgBQgluYXJyYXRpb24SDAABAgMEBQYHCAkKCxoMDA0ODxAREhMUFRYX",
            ),
        ],
    );
    let mut t = Tailer::new(&main);
    let blocks: Vec<_> = t
        .poll()
        .into_iter()
        .filter_map(|e| match e {
            TxEvent::Block(b) => Some((b.kind, b.text)),
            _ => None,
        })
        .collect();
    assert!(blocks.contains(&(BlockKind::Thinking, Some("private".into()))));
    assert!(blocks.contains(&(BlockKind::Text, Some("Checking the layout.".into()))));
    assert_eq!(t.last_text(), Some("Checking the layout."));
}

#[test]
fn usage_once_per_message_api_errors_and_turn_duration() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let usage = json!({"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100,
            "cache_creation_input_tokens": 40, "cache_creation": {"ephemeral_1h_input_tokens": 30}});
    let part = |uuid: &str, kind: &str| {
        json!({"type": "assistant", "uuid": uuid, "message": {"id": "msg_1", "model": "claude-sonnet-5-5",
                "role": "assistant", "usage": usage, "content": [{"type": kind, "text": "x", "thinking": ""}]}})
    };
    write_lines(
        &main,
        &[
            prompt("u1", None, "hi", 1),
            part("a1", "thinking"),
            part("a2", "text"),
            json!({"type": "assistant", "uuid": "e1", "isApiErrorMessage": true, "error": "rate_limit",
                    "message": {"id": "e", "model": "<synthetic>", "role": "assistant",
                        "content": [{"type": "text", "text": "API Error: 429"}]}}),
            json!({"type": "system", "subtype": "turn_duration", "uuid": "d1", "durationMs": 4200}),
        ],
    );
    let evs = Tailer::new(&main).poll();
    let usages: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            TxEvent::Usage { usage, .. } => Some(*usage),
            _ => None,
        })
        .collect();
    assert_eq!(usages.len(), 1, "one message, two entries");
    assert_eq!(
        (usages[0].cache_write_5m, usages[0].cache_write_1h),
        (10.0, 30.0)
    );
    assert!(evs.iter().any(|e| matches!(e, TxEvent::ApiError { kind: Some(k), message } if k == "rate_limit" && message == "API Error: 429")));
    assert!(evs.iter().any(|e| matches!(
        e,
        TxEvent::TurnEnd {
            duration_ms: Some(4200)
        }
    )));
}

#[test]
fn background_tasks_start_and_end_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let call = |id: &str, cmd: &str| {
        json!({"type": "assistant", "uuid": format!("c-{id}"), "message": {"id": format!("m-{id}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": "Bash",
                    "input": {"command": cmd, "description": "wait", "run_in_background": true}}]}})
    };
    let note = |task: &str| {
        format!(
            "<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n</task-notification>"
        )
    };
    write_lines(
        &main,
        &[
            call("t1", "sleep 5"),
            tool_result_entry(
                "t1",
                "Command running in background with ID: bg1. Output is being written to: /x",
                json!({}),
            ),
            call("t2", "sleep 6"),
            tool_result_entry(
                "t2",
                "Command running in background with ID: bg2. Output is being written to: /y",
                json!({}),
            ),
            // Ended while idle: a plain user message.
            json!({"type": "user", "uuid": "n1", "message": {"role": "user", "content": note("bg1")}}),
            // Ended while busy: a queued_command attachment.
            json!({"type": "attachment", "uuid": "n2", "attachment": {"type": "queued_command",
                    "commandMode": "task-notification", "prompt": note("bg2")}}),
        ],
    );
    let mut t = Tailer::new(&main);
    t.poll();
    let tasks = t.tasks();
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        (tasks[0].kind, tasks[0].command.as_deref()),
        (TaskKind::Shell, Some("sleep 5"))
    );
    assert!(tasks.iter().all(|t| t.status == "completed"), "{tasks:?}");
}

/// A Monitor is a task of its own kind; its events (notifications without a status) do not
/// end it, the one saying its stream ended does.
#[test]
fn a_monitor_runs_until_its_stream_ends() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let user = |uuid: &str, text: String| json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": text}});
    let note = |inner: &str| {
        format!("<task-notification>\n<task-id>bm1</task-id>\n{inner}\n</task-notification>")
    };
    let mut lines = vec![
        json!({"type": "assistant", "uuid": "c-t1", "message": {"id": "m-t1", "role": "assistant",
                "content": [{"type": "tool_use", "id": "t1", "name": "Monitor",
                    "input": {"command": "tail -F app.log", "description": "app errors", "timeout_ms": 900000}}]}}),
        tool_result_entry(
            "t1",
            "Monitor started (task bm1, timeout 900000ms). You will be notified on each event.",
            json!({"taskId": "bm1", "timeoutMs": 900000, "persistent": false}),
        ),
        user(
            "e1",
            note("<summary>Monitor event: \"app errors\"</summary>\n<event>ERROR boom</event>"),
        ),
    ];
    write_lines(&main, &lines);
    let mut t = Tailer::new(&main);
    t.poll();
    let task = &t.tasks()[0];
    assert_eq!(
        (
            task.kind,
            task.description.as_deref(),
            task.command.as_deref(),
            task.status.as_str()
        ),
        (
            TaskKind::Monitor,
            Some("app errors"),
            Some("tail -F app.log"),
            "running"
        )
    );
    // Appended: the stream ended.
    lines.push(user(
        "e2",
        note("<status>completed</status>\n<summary>Monitor \"app errors\" stream ended</summary>"),
    ));
    write_lines(&main, &lines);
    t.poll();
    assert_eq!(t.tasks()[0].status, "completed");
}

/// A plugin's monitor has no tool call: its notifications (an event, queued while busy or
/// sent as a prompt, or the end of its stream) name it a Monitor; a shell's do not.
#[test]
fn a_monitor_is_known_by_its_notifications() {
    let note = |id: &str, inner: &str| {
        format!("<task-notification>\n<task-id>{id}</task-id>\n{inner}\n</task-notification>")
    };
    assert_eq!(
        monitor_notice(&note(
            "b1",
            "<summary>Monitor event: \"haa Worker 事件\"</summary>\n<event>w1 DONE done</event>"
        )),
        Some("b1".into())
    );
    assert_eq!(
        monitor_notice(&note(
            "b2",
            "<status>completed</status>\n<summary>Monitor \"x\" stream ended</summary>"
        )),
        Some("b2".into())
    );
    assert_eq!(
        monitor_notice(&note(
            "b3",
            "<status>completed</status>\n<summary>Background command \"Monitor x\" completed (exit code 0)</summary>"
        )),
        None
    );
    assert_eq!(monitor_notice("Monitor event: \"x\""), None);

    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let event = note(
        "bq",
        "<summary>Monitor event: \"q\"</summary>\n<event>e</event>",
    );
    write_lines(
        &main,
        &[
            json!({"type": "attachment", "uuid": "a1", "attachment": {"type": "queued_command",
                    "commandMode": "task-notification", "prompt": event}}),
            json!({"type": "user", "uuid": "u1", "message": {"role": "user",
                    "content": note("bp", "<summary>Monitor event: \"p\"</summary>\n<event>e</event>")}}),
            json!({"type": "user", "uuid": "u2", "message": {"role": "user",
                    "content": note("bs", "<status>completed</status>\n<summary>Background command \"s\" completed (exit code 0)</summary>")}}),
        ],
    );
    let mut t = Tailer::new(&main);
    t.poll();
    let mut ids: Vec<&String> = t.monitors().iter().collect();
    ids.sort();
    assert_eq!(ids, ["bp", "bq"]);
}

/// A foreground command moved to the background on its timeout is a task too, and TaskStop
/// (which sends no notification for a shell) ends it.
#[test]
fn a_task_moved_to_the_background_and_one_stopped_with_task_stop() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let call = |id: &str, name: &str, input: Value| {
        json!({"type": "assistant", "uuid": format!("c-{id}"), "message": {"id": format!("m-{id}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}})
    };
    write_lines(
        &main,
        &[
            call(
                "t1",
                "Bash",
                json!({"command": "make e2e", "description": "E2E"}),
            ),
            tool_result_entry(
                "t1",
                "running…\nCommand did not complete within its 120s timeout and was moved to the background (ID: bg1). Output is being written to: /x",
                json!({}),
            ),
            call(
                "t2",
                "Bash",
                json!({"command": "sleep 60", "description": "wait", "run_in_background": true}),
            ),
            tool_result_entry(
                "t2",
                "Command running in background with ID: bg2. Output is being written to: /y",
                json!({}),
            ),
            call("t3", "TaskStop", json!({"task_id": "bg2"})),
            tool_result_entry(
                "t3",
                "{\"message\":\"Successfully stopped task: bg2 (sleep 60)\"}",
                json!({}),
            ),
        ],
    );
    let mut t = Tailer::new(&main);
    t.poll();
    let status: Vec<_> = t
        .tasks()
        .iter()
        .map(|t| (t.id.as_str(), t.status.as_str()))
        .collect();
    assert_eq!(status, [("bg1", "running"), ("bg2", "stopped")]);
}

/// A teammate is idle from its `idle_notification` (delivered at once or queued) until the
/// lead sends it a message; Stop's list names it by the description it was spawned with.
#[test]
fn a_teammate_is_idle_between_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let call = |id: &str, name: &str, input: Value| {
        json!({"type": "assistant", "uuid": format!("c-{id}"), "message": {"id": format!("m-{id}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}})
    };
    let relayed = |from: &str, at: &str| {
        format!(
            "Another Claude session sent a message:\n<teammate-message teammate_id=\"{from}\" color=\"blue\">\n{{\"type\":\"idle_notification\",\"from\":\"{from}\",\"timestamp\":\"{at}\",\"idleReason\":\"available\"}}\n</teammate-message>"
        )
    };
    let spawn = call(
        "t1",
        "Agent",
        json!({"name": "w", "description": "write docs", "prompt": "p"}),
    );
    let mut lines = vec![spawn];
    write_lines(&main, &lines);
    let mut t = Tailer::new(&main);
    t.poll();
    assert!(!t.teammate_idle("write docs"));
    assert!(!t.teammate_idle("something else"));

    lines.push(json!({"type": "user", "uuid": "n1", "message": {"role": "user", "content": relayed("w", "2026-11-09T09:37:52.717Z")}}));
    write_lines(&main, &lines);
    t.poll();
    assert!(t.teammate_idle("write docs"));

    let mut send = call("t2", "SendMessage", json!({"to": "w", "message": "more"}));
    send["timestamp"] = json!("2026-11-09T09:38:00.000Z");
    lines.push(send);
    write_lines(&main, &lines);
    t.poll();
    assert!(!t.teammate_idle("write docs"));

    lines.push(
            json!({"type": "attachment", "uuid": "n2", "attachment": {"type": "queued_command",
            "prompt": relayed("w", "2026-11-09T09:37:59.000Z"), "origin": {"kind": "coordinator"}}}),
        );
    write_lines(&main, &lines);
    t.poll();
    // Written after the lead's message but sent before it: the new round is still running.
    assert!(!t.teammate_idle("write docs"));

    lines.push(
        json!({"type": "user", "uuid": "n3", "message": {"role": "user",
            "content": relayed("w", "2026-11-09T09:39:00.000Z")}}),
    );
    write_lines(&main, &lines);
    t.poll();
    assert!(t.teammate_idle("write docs"));
}

/// `tasks` / `get_tasks` put `BgTask` on the wire: the kind is a lowercase string, every field
/// is there (`null` / `false` when there is nothing to say).
#[test]
fn background_tasks_serialize_as_on_the_wire() {
    let task = |kind, tool_use_id: Option<&str>, command: Option<&str>| BgTask {
        id: "t1".into(),
        kind,
        tool_use_id: tool_use_id.map(str::to_string),
        description: Some("d".into()),
        command: command.map(str::to_string),
        status: "running".into(),
        idle: false,
    };
    let all = [
        task(TaskKind::Shell, Some("toolu_1"), Some("sleep 5")),
        task(TaskKind::Agent, None, None),
        task(TaskKind::Monitor, Some("toolu_1"), Some("tail -F x")),
        task(TaskKind::Workflow, Some("toolu_1"), None),
    ];
    assert_eq!(
        serde_json::to_string(&all).unwrap(),
        concat!(
            r#"[{"id":"t1","kind":"shell","tool_use_id":"toolu_1","description":"d","command":"sleep 5","status":"running","idle":false},"#,
            r#"{"id":"t1","kind":"agent","tool_use_id":null,"description":"d","command":null,"status":"running","idle":false},"#,
            r#"{"id":"t1","kind":"monitor","tool_use_id":"toolu_1","description":"d","command":"tail -F x","status":"running","idle":false},"#,
            r#"{"id":"t1","kind":"workflow","tool_use_id":"toolu_1","description":"d","command":null,"status":"running","idle":false}]"#
        )
    );
}

#[test]
fn a_compact_boundary_is_not_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let (_, evs) = tail_then(
        &main,
        &history(),
        &[
            system("c1", None, "compact_boundary", Some("a2")),
            prompt("u3", Some("c1"), "after compact", 5),
        ],
    );
    assert!(
        !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
        "{evs:?}"
    );
}

#[test]
fn a_child_written_before_its_parent_is_not_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    // Read together: the teammate prompt comes first, then the entry it hangs off.
    let (_, evs) = tail_then(
        &main,
        &history(),
        &[
            prompt("t1", Some("h1"), "teammate says hi", 5),
            system("h1", Some("a2"), "stop_hook_summary", None),
        ],
    );
    assert!(
        !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
        "{evs:?}"
    );
    let texts: Vec<_> = main_blocks(&main, 0)
        .0
        .into_iter()
        .filter_map(|b| b.text)
        .collect();
    assert_eq!(texts.last().map(String::as_str), Some("teammate says hi"));
    // Read apart: the parent is not there yet when the child is read.
    let main2 = dir.path().join("s2.jsonl");
    let mut lines = history();
    lines.push(prompt("t1", Some("h1"), "teammate says hi", 5));
    let (mut t, evs) = tail_then(&main2, &history(), &lines[history().len()..]);
    assert!(!evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })));
    assert!(
        main_blocks(&main2, 0).0.len() >= 4,
        "the conversation is still there"
    );
    lines.push(system("h1", Some("a2"), "stop_hook_summary", None));
    write_lines(&main2, &lines);
    assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
}

#[test]
fn resending_the_same_prompt_is_not_a_branch_and_cursors_still_work() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let first = vec![
        prompt("u1", None, "first", 1),
        reply("a1", "u1", "one", 2),
        prompt("u2", Some("a1"), "again", 3),
        interruption("i2", "u2"),
    ];
    write_lines(&main, &first);
    let (_, cursor) = main_blocks(&main, 0);
    let (_, evs) = tail_then(&main, &first, &[prompt("u3", Some("a1"), "again", 4)]);
    assert!(
        !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
        "{evs:?}"
    );
    let after: Vec<_> = main_blocks(&main, cursor)
        .0
        .into_iter()
        .map(|b| b.uuid)
        .collect();
    assert_eq!(after, ["u3"]);
}

fn tool_result_entry(tool_use_id: &str, content: &str, extra: Value) -> Value {
    let mut v = json!({"type": "user", "uuid": format!("r-{tool_use_id}"),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id, "content": content}]}});
    if let (Value::Object(m), Value::Object(x)) = (&mut v, extra) {
        m.extend(x);
    }
    v
}

#[test]
fn persisted_output_points_only_into_the_sessions_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let results = dir.path().join("s").join("tool-results");
    std::fs::create_dir_all(&results).unwrap();
    let full = results.join("abc.txt");
    std::fs::write(&full, "FULL OUTPUT").unwrap();
    let preview = |p: &Path| {
        format!(
            "<persisted-output>\nOutput too large (42.4KB). Full output saved to: {}\n\nPreview (first 2KB):\nFULL",
            p.display()
        )
    };
    let outside = dir.path().join("elsewhere.txt");
    std::fs::write(&outside, "SECRET").unwrap();
    write_lines(
        &main,
        &[
            tool_result_entry("t1", &preview(&full), json!({})),
            tool_result_entry("t2", &preview(&outside), json!({})),
            tool_result_entry("t3", "short", json!({})),
        ],
    );
    let mut t = Tailer::new(&main);
    let evs = t.poll();
    let bs = blocks(&evs);
    let p = bs[0].persisted.as_ref().unwrap();
    assert_eq!((p.path.as_str(), p.size), (full.to_str().unwrap(), 11));
    assert!(
        bs[0]
            .content
            .as_deref()
            .unwrap()
            .starts_with("<persisted-output>")
    );
    assert!(
        bs[1].persisted.is_none(),
        "a path outside tool-results/ is not followed"
    );
    assert_eq!(t.tool_result("t1").unwrap().0, "FULL OUTPUT");
    assert!(
        t.tool_result("t2")
            .unwrap()
            .0
            .starts_with("<persisted-output>")
    );
    assert_eq!(t.tool_result("t3").unwrap(), ("short".to_string(), None));
    assert!(t.tool_result("nope").is_none());
}

#[test]
fn workflow_runs_are_reported_with_their_subagent_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("s.jsonl");
    let sdir = dir.path().join("s");
    std::fs::create_dir_all(sdir.join("workflows")).unwrap();
    let agents = sdir.join("subagents").join("workflows").join("wf_1");
    std::fs::create_dir_all(&agents).unwrap();
    let wf = |status: &str, state: &str| {
        json!({"workflowName": "review", "status": status,
                "phases": [{"title": "Read", "detail": "parallel"}],
                "workflowProgress": [
                    {"type": "workflow_phase", "index": 1, "title": "Read"},
                    {"type": "workflow_agent", "agentId": "ag1", "label": "reader",
                        "phaseIndex": 1, "state": state}]})
        .to_string()
    };
    std::fs::write(sdir.join("workflows/wf_1.json"), wf("running", "running")).unwrap();
    write_lines(
        &main,
        &[tool_result_entry(
            "toolu_wf",
            "started",
            json!({"toolUseResult": {"runId": "wf_1"}}),
        )],
    );
    write_lines(
        &agents.join("agent-ag1.jsonl"),
        &[prompt("x1", None, "read it", 1)],
    );
    let mut t = Tailer::new(&main);
    let evs = t.poll();
    let w = evs
        .iter()
        .find_map(|e| match e {
            TxEvent::Workflow(v) => Some(v.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(w["run_id"], "wf_1");
    assert_eq!(w["tool_use_id"], "toolu_wf");
    assert_eq!(w["status"], "running");
    assert_eq!(w["phases"][0]["title"], "Read");
    assert_eq!(w["agents"][0]["label"], "reader");
    let sub = blocks(&evs)
        .into_iter()
        .find(|b| b.agent_id.as_deref() == Some("ag1"))
        .unwrap();
    assert_eq!(sub.run_id.as_deref(), Some("wf_1"));
    assert_eq!(sub.parent_tool_use_id.as_deref(), Some("toolu_wf"));
    // Unchanged files: nothing new; a new state: a new snapshot.
    t.last_discover = None;
    assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Workflow(_))));
    std::fs::write(sdir.join("workflows/wf_1.json"), wf("completed", "done")).unwrap();
    t.last_discover = None;
    let w = t
        .poll()
        .into_iter()
        .find_map(|e| match e {
            TxEvent::Workflow(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert_eq!(w["status"], "completed");
}

fn queue_texts(t: &Tailer) -> Vec<String> {
    t.queue().iter().map(|q| q.text.clone()).collect()
}
