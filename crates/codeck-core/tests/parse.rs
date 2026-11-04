use std::io::Write;

use codeck_core::parse::{parse_bytes, read_entries};
use codeck_core::raw::RawContent;

const USER: &str = r#"{"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-10-07T09:34:56.789Z","message":{"role":"user","content":"hello"}}"#;
const ASSISTANT: &str = r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"msg_1","model":"claude-opus","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":3,"output_tokens":5,"cache_read_input_tokens":7,"cache_creation_input_tokens":11}}}"#;

#[test]
fn trailing_partial_line_is_not_consumed_until_completed() {
    let partial = &ASSISTANT[..40];
    let buf = format!("{USER}\n{partial}");
    let chunk = parse_bytes(buf.as_bytes(), 0);
    assert_eq!(chunk.records.len(), 1);
    assert_eq!(chunk.failed, 0);
    assert_eq!(chunk.consumed_to, USER.len() as u64 + 1);

    // Claude Code finishes writing the line; the next read resumes at `consumed_to`.
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(buf.as_bytes()).unwrap();
    file.write_all(format!("{}\n", &ASSISTANT[40..]).as_bytes())
        .unwrap();
    file.flush().unwrap();
    let next = read_entries(file.path(), chunk.consumed_to).unwrap();
    assert_eq!(next.records.len(), 1);
    assert_eq!(next.records[0].offset, chunk.consumed_to);
    assert_eq!(next.records[0].entry.uuid.as_deref(), Some("a1"));
    assert_eq!(next.consumed_to, (USER.len() + ASSISTANT.len() + 2) as u64);

    // Nothing new: an empty chunk that stays at the end.
    let again = read_entries(file.path(), next.consumed_to).unwrap();
    assert!(again.records.is_empty());
    assert_eq!(again.consumed_to, next.consumed_to);
}

#[test]
fn corrupt_line_is_counted_and_skipped() {
    let buf = format!("{USER}\n{{\"type\":\"user\",\"uuid\":\n\n[1,2]\n{ASSISTANT}\n");
    let chunk = parse_bytes(buf.as_bytes(), 0);
    let uuids: Vec<_> = chunk
        .records
        .iter()
        .map(|r| r.entry.uuid.as_deref().unwrap())
        .collect();
    assert_eq!(uuids, ["u1", "a1"]);
    assert_eq!(
        chunk.failed, 2,
        "truncated object and non-object line; blank line is not a failure"
    );
    let corrupt_offset = USER.len() + 1;
    assert!(
        chunk
            .first_error
            .unwrap()
            .contains(&format!("字节偏移 {corrupt_offset}"))
    );
    assert_eq!(chunk.records[1].line_no, 5);
    assert_eq!(chunk.consumed_to, buf.len() as u64);
}

#[test]
fn offsets_are_exact_across_multibyte_utf8() {
    let cjk =
        r#"{"type":"user","uuid":"u0","message":{"role":"user","content":"中文提问 🎉 émoji"}}"#;
    let buf = format!("{cjk}\r\n{USER}\n{ASSISTANT}\n");
    let base = 1_000;
    let chunk = parse_bytes(buf.as_bytes(), base);
    assert_eq!(chunk.records.len(), 3);
    for rec in &chunk.records {
        let start = (rec.offset - base) as usize;
        assert!(
            buf[start..].starts_with('{'),
            "offset {} is a line start",
            rec.offset
        );
    }
    assert_eq!(chunk.records[1].offset, base + cjk.len() as u64 + 2);
    assert_eq!(
        chunk.records[2].offset,
        base + (cjk.len() + 2 + USER.len() + 1) as u64
    );
    assert_eq!(
        chunk.records[0].entry.content_text(),
        Some("中文提问 🎉 émoji")
    );
    assert_eq!(chunk.consumed_to, base + buf.len() as u64);
}

#[test]
fn unknown_fields_and_types_are_tolerated() {
    let lines = [
        // Unknown entry type with unknown fields.
        r#"{"type":"brand-new-entry","uuid":"x1","someField":{"deep":[1,2,3]},"v":2}"#,
        // Known fields with unexpected JSON types become None instead of failing the line.
        r#"{"type":"user","uuid":123,"isMeta":"yes","parentUuid":["p"],"timestamp":42,"message":"plain","forkedFrom":"s"}"#,
        // Unknown block type, a non-object block, wrong-typed usage, and raw payloads kept as-is.
        r#"{"type":"assistant","uuid":"a9","message":{"id":"m9","content":[{"type":"hologram","payload":{"x":1}},"stray",{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}},{"type":"text","text":"done"}],"usage":{"input_tokens":"many","output_tokens":9}},"toolUseResult":"Error: denied"}"#,
        // Value types that are neither object nor string for content.
        r#"{"type":"user","uuid":"u9","message":{"content":17},"toolUseResult":{"agentId":"ag1","status":"async_launched","agent_id":"ag2","extra":true}}"#,
    ];
    let buf = lines.join("\n") + "\n";
    let chunk = parse_bytes(buf.as_bytes(), 0);
    assert_eq!(chunk.failed, 0, "{:?}", chunk.first_error);
    assert_eq!(chunk.records.len(), 4);

    let unknown = &chunk.records[0].entry;
    assert_eq!(unknown.entry_type(), "brand-new-entry");
    assert_eq!(unknown.uuid.as_deref(), Some("x1"));

    let mistyped = &chunk.records[1].entry;
    assert_eq!(mistyped.uuid, None);
    assert_eq!(mistyped.is_meta, None);
    assert_eq!(mistyped.parent_uuid, None);
    assert_eq!(mistyped.timestamp_ms(), None);
    assert!(mistyped.message.is_none());
    assert!(mistyped.forked_from.is_none());

    let assistant = &chunk.records[2].entry;
    let blocks = assistant.blocks();
    assert_eq!(blocks.len(), 3, "the non-object block is dropped");
    assert_eq!(blocks[0].block_type(), "hologram");
    assert_eq!(blocks[1].name.as_deref(), Some("Bash"));
    assert_eq!(
        blocks[1].input.as_ref().unwrap().get(),
        r#"{"command":"ls -la"}"#
    );
    assert_eq!(blocks[2].text_str().as_deref(), Some("done"));
    let usage = assistant.message.as_ref().unwrap().usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, Some(9.0));
    assert!(
        assistant.is_tool_result(),
        "string toolUseResult still marks a tool result"
    );
    assert!(assistant.tool_use_result_fields().is_none());

    let user = &chunk.records[3].entry;
    assert!(user.message.as_ref().unwrap().content.is_none());
    let tur = user.tool_use_result_fields().unwrap();
    assert_eq!(tur.agent_id.as_deref(), Some("ag1"));
    assert_eq!(tur.agent_id_snake.as_deref(), Some("ag2"));
    assert_eq!(tur.status.as_deref(), Some("async_launched"));
}

#[test]
fn known_fields_are_parsed() {
    let buf = format!("{USER}\n{ASSISTANT}\n");
    let chunk = parse_bytes(buf.as_bytes(), 0);
    let user = &chunk.records[0].entry;
    assert_eq!(user.entry_type(), "user");
    assert_eq!(user.parent_uuid, None);
    assert_eq!(user.timestamp_ms(), Some(1_791_365_696_789.0));
    let a = chunk.records[1].entry.message.as_ref().unwrap();
    assert_eq!(a.id.as_deref(), Some("msg_1"));
    assert!(matches!(a.content, Some(RawContent::Blocks(ref b)) if b.len() == 1));
    let u = a.usage.as_ref().unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.cache_read_input_tokens,
            u.cache_creation_input_tokens
        ),
        (Some(3.0), Some(5.0), Some(7.0), Some(11.0))
    );
}
