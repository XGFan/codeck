//! JSONL 协议层：把一行 JSON 解析成命令，把响应 / 事件写成一行 JSON。
//! 只做纯编解码，不碰 PTY、进程与 transcript。

use std::io::Write;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Map, Value, json};

use crate::pty::key_bytes;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    AllowAlways,
    Deny,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Options(Vec<String>),
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Named(String),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Prompt {
        text: String,
    },
    Steer {
        text: String,
    },
    FollowUp {
        text: String,
    },
    Abort,
    AnswerPermission {
        request_id: String,
        decision: Decision,
        message: Option<String>,
    },
    AnswerQuestion {
        request_id: String,
        answers: Vec<Answer>,
    },
    Slash {
        command: String,
    },
    SetModel {
        model: String,
    },
    Compact {
        instructions: Option<String>,
    },
    Clear,
    SendKeys {
        keys: Vec<Key>,
    },
    GetState,
    GetMessages {
        since: Option<u64>,
    },
    GetScreen,
    GetToolResult {
        tool_use_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: Option<String>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub id: Option<String>,
    pub command: Option<String>,
    pub code: &'static str,
    pub message: String,
}

fn bad_request(id: Option<String>, command: Option<String>, message: String) -> RpcError {
    RpcError {
        id,
        command,
        code: "bad_request",
        message,
    }
}

/// 必填的非空字符串字段（去掉首尾空白后为空也算缺失）。
fn req_text(obj: &Map<String, Value>, cmd: &str, field: &str) -> Result<String, String> {
    match obj.get(field) {
        None | Some(Value::Null) => Err(format!("{cmd} 缺少 {field}")),
        Some(Value::String(s)) if s.trim().is_empty() => Err(format!("{cmd} 的 {field} 不能为空")),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("{cmd} 的 {field} 必须是字符串")),
    }
}

fn opt_text(obj: &Map<String, Value>, cmd: &str, field: &str) -> Result<Option<String>, String> {
    match obj.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{cmd} 的 {field} 必须是字符串")),
    }
}

fn parse_decision(obj: &Map<String, Value>) -> Result<Decision, String> {
    match obj.get("decision") {
        None | Some(Value::Null) => Err("answer_permission 缺少 decision".into()),
        Some(Value::String(s)) => match s.as_str() {
            "allow" => Ok(Decision::Allow),
            "allow_always" => Ok(Decision::AllowAlways),
            "deny" => Ok(Decision::Deny),
            other => Err(format!(
                "answer_permission 的 decision 只能是 allow / allow_always / deny，收到 {other}"
            )),
        },
        Some(_) => Err("answer_permission 的 decision 必须是字符串".into()),
    }
}

fn parse_answer(item: &Value, cmd: &str, index: usize) -> Result<Answer, String> {
    let obj = item
        .as_object()
        .ok_or_else(|| format!("{cmd} 的 answers[{index}] 必须是对象"))?;
    if obj.len() != 1 {
        return Err(format!(
            "{cmd} 的 answers[{index}] 必须恰好有一个 options 或 other 字段"
        ));
    }
    if let Some(value) = obj.get("options") {
        let items = value
            .as_array()
            .ok_or_else(|| format!("{cmd} 的 answers[{index}].options 必须是数组"))?;
        if items.is_empty() {
            return Err(format!("{cmd} 的 answers[{index}].options 不能为空"));
        }
        let mut labels = Vec::with_capacity(items.len());
        for label in items {
            match label.as_str() {
                Some(s) if !s.is_empty() => labels.push(s.to_string()),
                _ => {
                    return Err(format!(
                        "{cmd} 的 answers[{index}].options 里必须都是非空字符串"
                    ));
                }
            }
        }
        return Ok(Answer::Options(labels));
    }
    if let Some(value) = obj.get("other") {
        return match value.as_str() {
            Some(s) if !s.is_empty() => Ok(Answer::Other(s.to_string())),
            _ => Err(format!("{cmd} 的 answers[{index}].other 必须是非空字符串")),
        };
    }
    Err(format!(
        "{cmd} 的 answers[{index}] 必须恰好有一个 options 或 other 字段"
    ))
}

fn parse_key(item: &Value, index: usize) -> Result<Key, String> {
    match item {
        Value::String(name) => match key_bytes(name) {
            Some(_) => Ok(Key::Named(name.clone())),
            None => Err(format!("send_keys 的 keys[{index}] 不是已知按键：{name}")),
        },
        Value::Object(obj) => {
            if let Some(value) = obj.get("text") {
                return match value.as_str() {
                    Some(s) => Ok(Key::Text(s.to_string())),
                    None => Err(format!("send_keys 的 keys[{index}].text 必须是字符串")),
                };
            }
            if let Some(value) = obj.get("bytes") {
                let encoded = value.as_str().ok_or_else(|| {
                    format!("send_keys 的 keys[{index}].bytes 必须是 base64 字符串")
                })?;
                let bytes = BASE64
                    .decode(encoded)
                    .map_err(|_| format!("send_keys 的 keys[{index}].bytes 不是合法 base64"))?;
                return Ok(Key::Bytes(bytes));
            }
            Err(format!(
                "send_keys 的 keys[{index}] 必须是字符串、{{\"text\":…}} 或 {{\"bytes\":…}}"
            ))
        }
        _ => Err(format!("send_keys 的 keys[{index}] 格式不对")),
    }
}

fn parse_command(kind: &str, obj: &Map<String, Value>) -> Result<Command, String> {
    match kind {
        "prompt" => Ok(Command::Prompt {
            text: req_text(obj, kind, "text")?,
        }),
        "steer" => Ok(Command::Steer {
            text: req_text(obj, kind, "text")?,
        }),
        "follow_up" => Ok(Command::FollowUp {
            text: req_text(obj, kind, "text")?,
        }),
        "abort" => Ok(Command::Abort),
        "answer_permission" => {
            let request_id = req_text(obj, kind, "request_id")?;
            let decision = parse_decision(obj)?;
            let message = opt_text(obj, kind, "message")?;
            if message.is_some() && decision != Decision::Deny {
                return Err("answer_permission 的 message 只能和 deny 一起用".into());
            }
            Ok(Command::AnswerPermission {
                request_id,
                decision,
                message,
            })
        }
        "answer_question" => {
            let request_id = req_text(obj, kind, "request_id")?;
            let items = match obj.get("answers") {
                None | Some(Value::Null) => return Err("answer_question 缺少 answers".into()),
                Some(Value::Array(a)) => a,
                Some(_) => return Err("answer_question 的 answers 必须是数组".into()),
            };
            if items.is_empty() {
                return Err("answer_question 的 answers 不能为空".into());
            }
            let mut answers = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                answers.push(parse_answer(item, kind, index)?);
            }
            Ok(Command::AnswerQuestion {
                request_id,
                answers,
            })
        }
        "slash" => {
            let raw = req_text(obj, kind, "command")?;
            let command = raw.trim().trim_start_matches('/').to_string();
            if command.trim().is_empty() {
                return Err("slash 的 command 不能只有 /".into());
            }
            Ok(Command::Slash { command })
        }
        "set_model" => Ok(Command::SetModel {
            model: req_text(obj, kind, "model")?,
        }),
        "compact" => Ok(Command::Compact {
            instructions: opt_text(obj, kind, "instructions")?,
        }),
        "clear" => Ok(Command::Clear),
        "send_keys" => {
            let items = match obj.get("keys") {
                None | Some(Value::Null) => return Err("send_keys 缺少 keys".into()),
                Some(Value::Array(a)) => a,
                Some(_) => return Err("send_keys 的 keys 必须是数组".into()),
            };
            if items.is_empty() {
                return Err("send_keys 的 keys 不能为空".into());
            }
            let mut keys = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                keys.push(parse_key(item, index)?);
            }
            Ok(Command::SendKeys { keys })
        }
        "get_state" => Ok(Command::GetState),
        "get_messages" => {
            let since = match obj.get("since") {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) => match n.as_u64() {
                    Some(n) => Some(n),
                    None => return Err("get_messages 的 since 必须是非负整数".into()),
                },
                Some(_) => return Err("get_messages 的 since 必须是非负整数".into()),
            };
            Ok(Command::GetMessages { since })
        }
        "get_screen" => Ok(Command::GetScreen),
        "get_tool_result" => Ok(Command::GetToolResult {
            tool_use_id: req_text(obj, kind, "tool_use_id")?,
        }),
        other => Err(format!("unknown::{other}")),
    }
}

pub fn parse_line(line: &str) -> Result<Request, RpcError> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return Err(bad_request(None, None, "不是合法的 JSON".into())),
    };
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Err(bad_request(None, None, "每行必须是一个 JSON 对象".into())),
    };
    let id = match obj.get("id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(_) => return Err(bad_request(None, None, "id 必须是字符串或数字".into())),
    };
    let kind = match obj.get("type") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(bad_request(id, None, "缺少 type 或 type 不是字符串".into())),
    };
    match parse_command(&kind, obj) {
        Ok(command) => Ok(Request { id, command }),
        Err(message) => match message.strip_prefix("unknown::") {
            Some(unknown) => Err(RpcError {
                id,
                command: Some(unknown.to_string()),
                code: "unknown_command",
                message: format!("不认识的命令：{unknown}"),
            }),
            None => Err(bad_request(id, Some(kind), message)),
        },
    }
}

pub fn command_name(c: &Command) -> &'static str {
    match c {
        Command::Prompt { .. } => "prompt",
        Command::Steer { .. } => "steer",
        Command::FollowUp { .. } => "follow_up",
        Command::Abort => "abort",
        Command::AnswerPermission { .. } => "answer_permission",
        Command::AnswerQuestion { .. } => "answer_question",
        Command::Slash { .. } => "slash",
        Command::SetModel { .. } => "set_model",
        Command::Compact { .. } => "compact",
        Command::Clear => "clear",
        Command::SendKeys { .. } => "send_keys",
        Command::GetState => "get_state",
        Command::GetMessages { .. } => "get_messages",
        Command::GetScreen => "get_screen",
        Command::GetToolResult { .. } => "get_tool_result",
    }
}

pub fn key_to_bytes(k: &Key) -> Vec<u8> {
    match k {
        Key::Named(name) => key_bytes(name).map(<[u8]>::to_vec).unwrap_or_default(),
        Key::Text(text) => text.as_bytes().to_vec(),
        Key::Bytes(bytes) => bytes.clone(),
    }
}

pub fn response_ok(id: Option<&str>, command: &str, data: Value) -> Value {
    json!({
        "type": "response",
        "id": id,
        "command": command,
        "success": true,
        "data": data,
    })
}

pub fn response_err(id: Option<&str>, command: &str, code: &str, message: &str) -> Value {
    json!({
        "type": "response",
        "id": id,
        "command": command,
        "success": false,
        "error": { "code": code, "message": message },
    })
}

/// 线程安全地往 stdout 写一行 JSON。
#[derive(Clone)]
pub struct Out {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Out {
    pub fn stdout() -> Out {
        Out::new(Box::new(std::io::stdout()))
    }

    pub fn new(w: Box<dyn Write + Send>) -> Out {
        Out {
            writer: Arc::new(Mutex::new(w)),
        }
    }

    /// 紧凑 JSON + `\n`，写完 flush；写失败静默忽略。
    pub fn send(&self, v: &Value) {
        let mut line = match serde_json::to_vec(v) {
            Ok(line) => line,
            Err(_) => return,
        };
        line.push(b'\n');
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(&line);
            let _ = w.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Request {
        parse_line(line).expect("应该解析成功")
    }

    fn err(line: &str) -> RpcError {
        parse_line(line).expect_err("应该解析失败")
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(
            parse(r#"{"id":"1","type":"prompt","text":"hi"}"#),
            Request {
                id: Some("1".into()),
                command: Command::Prompt { text: "hi".into() },
            }
        );
        assert_eq!(
            parse(r#"{"type":"steer","text":"stop"}"#),
            Request {
                id: None,
                command: Command::Steer {
                    text: "stop".into()
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"follow_up","text":"next"}"#),
            Request {
                id: None,
                command: Command::FollowUp {
                    text: "next".into()
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"abort"}"#),
            Request {
                id: None,
                command: Command::Abort,
            }
        );
        assert_eq!(
            parse(r#"{"type":"answer_permission","request_id":"t1","decision":"allow"}"#),
            Request {
                id: None,
                command: Command::AnswerPermission {
                    request_id: "t1".into(),
                    decision: Decision::Allow,
                    message: None,
                },
            }
        );
        assert_eq!(
            parse(
                r#"{"type":"answer_permission","request_id":"t1","decision":"deny","message":"no"}"#
            ),
            Request {
                id: None,
                command: Command::AnswerPermission {
                    request_id: "t1".into(),
                    decision: Decision::Deny,
                    message: Some("no".into()),
                },
            }
        );
        assert_eq!(
            parse(
                r#"{"type":"answer_question","request_id":"q1","answers":[{"options":["a","b"]},{"other":"x"}]}"#
            ),
            Request {
                id: None,
                command: Command::AnswerQuestion {
                    request_id: "q1".into(),
                    answers: vec![
                        Answer::Options(vec!["a".into(), "b".into()]),
                        Answer::Other("x".into()),
                    ],
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"slash","command":"/model sonnet"}"#),
            Request {
                id: None,
                command: Command::Slash {
                    command: "model sonnet".into()
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"set_model","model":"sonnet"}"#),
            Request {
                id: None,
                command: Command::SetModel {
                    model: "sonnet".into()
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"compact","instructions":"focus on tests"}"#),
            Request {
                id: None,
                command: Command::Compact {
                    instructions: Some("focus on tests".into())
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"compact"}"#),
            Request {
                id: None,
                command: Command::Compact { instructions: None },
            }
        );
        assert_eq!(
            parse(r#"{"type":"clear"}"#),
            Request {
                id: None,
                command: Command::Clear,
            }
        );
        assert_eq!(
            parse(r#"{"type":"send_keys","keys":["esc",{"text":"abc"},{"bytes":"AQI="}]}"#),
            Request {
                id: None,
                command: Command::SendKeys {
                    keys: vec![
                        Key::Named("esc".into()),
                        Key::Text("abc".into()),
                        Key::Bytes(vec![1, 2]),
                    ],
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"get_state"}"#),
            Request {
                id: None,
                command: Command::GetState,
            }
        );
        assert_eq!(
            parse(r#"{"type":"get_messages","since":7}"#),
            Request {
                id: None,
                command: Command::GetMessages { since: Some(7) },
            }
        );
        assert_eq!(
            parse(r#"{"type":"get_messages"}"#),
            Request {
                id: None,
                command: Command::GetMessages { since: None },
            }
        );
        assert_eq!(
            parse(r#"{"type":"get_tool_result","tool_use_id":"toolu_1"}"#),
            Request {
                id: None,
                command: Command::GetToolResult {
                    tool_use_id: "toolu_1".into(),
                },
            }
        );
        assert_eq!(
            parse(r#"{"type":"get_screen"}"#),
            Request {
                id: None,
                command: Command::GetScreen,
            }
        );
    }

    #[test]
    fn numeric_id_becomes_decimal_string() {
        let req = parse(r#"{"id":42,"type":"get_state"}"#);
        assert_eq!(req.id.as_deref(), Some("42"));
        assert_eq!(command_name(&req.command), "get_state");
    }

    #[test]
    fn unknown_command_keeps_id() {
        let e = err(r#"{"id":"9","type":"nope"}"#);
        assert_eq!(e.id.as_deref(), Some("9"));
        assert_eq!(e.command.as_deref(), Some("nope"));
        assert_eq!(e.code, "unknown_command");
    }

    #[test]
    fn rejects_bad_lines() {
        let e = err("not json");
        assert_eq!((e.code, e.id), ("bad_request", None));

        let e = err(r#"{"id":"1"}"#);
        assert_eq!((e.code, e.id.as_deref()), ("bad_request", Some("1")));

        let e = err(r#"{"type":"prompt"}"#);
        assert_eq!(e.code, "bad_request");
        assert_eq!(e.command.as_deref(), Some("prompt"));

        let e = err(r#"{"type":"prompt","text":"   "}"#);
        assert_eq!(e.code, "bad_request");

        let e = err(r#"{"type":"answer_permission","request_id":"t","decision":"maybe"}"#);
        assert_eq!(e.code, "bad_request");

        let e = err(
            r#"{"type":"answer_permission","request_id":"t","decision":"allow","message":"x"}"#,
        );
        assert_eq!(e.code, "bad_request");

        let e = err(r#"{"type":"answer_question","request_id":"q","answers":[{"options":[]}]}"#);
        assert_eq!(e.code, "bad_request");

        let e = err(
            r#"{"type":"answer_question","request_id":"q","answers":[{"options":["a"],"other":"b"}]}"#,
        );
        assert_eq!(e.code, "bad_request");

        let e = err(r#"{"type":"send_keys","keys":["hyper"]}"#);
        assert_eq!(e.code, "bad_request");

        let e = err(r#"{"type":"send_keys","keys":[{"bytes":"!!!!"}]}"#);
        assert_eq!(e.code, "bad_request");
    }

    #[test]
    fn key_to_bytes_three_kinds() {
        assert_eq!(key_to_bytes(&Key::Named("enter".into())), b"\r".to_vec());
        assert_eq!(
            key_to_bytes(&Key::Text("é".into())),
            "é".as_bytes().to_vec()
        );
        assert_eq!(key_to_bytes(&Key::Bytes(vec![0, 255])), vec![0, 255]);
    }

    #[test]
    fn response_shapes() {
        let ok = response_ok(Some("1"), "prompt", json!({"uuid": "u"}));
        assert_eq!(
            ok,
            json!({
                "type": "response",
                "id": "1",
                "command": "prompt",
                "success": true,
                "data": {"uuid": "u"},
            })
        );

        let failed = response_err(None, "abort", "busy", "当前不忙");
        assert_eq!(
            failed,
            json!({
                "type": "response",
                "id": null,
                "command": "abort",
                "success": false,
                "error": {"code": "busy", "message": "当前不忙"},
            })
        );
    }

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn out_writes_one_line_with_newline() {
        let shared = Shared::default();
        let out = Out::new(Box::new(shared.clone()));
        out.send(&json!({"type": "ready", "protocol": 1}));
        let text = String::from_utf8(shared.0.lock().unwrap().clone()).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
        let value: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(value, json!({"type": "ready", "protocol": 1}));
    }
}
