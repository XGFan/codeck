//! DDL and schema version.

/// `PRAGMA user_version`; any mismatch deletes and recreates the index.
pub const SCHEMA_VERSION: u32 = 12;

/// `files.role`.
pub const ROLE_MAIN: i64 = 0;
pub const ROLE_SUBAGENT: i64 = 1;
pub const ROLE_WORKFLOW_SUBAGENT: i64 = 2;

/// `messages.role`.
pub const MSG_PROMPT: i64 = 0;
pub const MSG_ASSISTANT: i64 = 1;

/// `diag_counts.category`.
pub const DIAG_ENTRY: i64 = 0;
pub const DIAG_BLOCK: i64 = 1;
pub const DIAG_TOOL: i64 = 2;
pub const DIAG_SYSTEM_SUBTYPE: i64 = 3;

/// Bytes of a file's start kept in `files.head` for rewrite detection.
pub const HEAD_LEN: usize = 256;

/// Every table, index and trigger of the index. `user_version` is set separately.
pub const DDL: &str = r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE files (
  id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
  dev INTEGER NOT NULL, ino INTEGER NOT NULL, role INTEGER NOT NULL,
  session_id TEXT NOT NULL, agent_id TEXT,
  size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  head BLOB NOT NULL,
  parsed_offset INTEGER NOT NULL,
  text_offset INTEGER NOT NULL DEFAULT 0,
  line_count INTEGER NOT NULL DEFAULT 0, failed_lines INTEGER NOT NULL DEFAULT 0, first_error TEXT);
CREATE INDEX files_session ON files(session_id);
CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL, display_name TEXT NOT NULL, missing INTEGER NOT NULL);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY, project_id TEXT NOT NULL, cwd TEXT, title TEXT NOT NULL, title_source INTEGER NOT NULL,
  first_prompt TEXT, created_ms INTEGER, last_active_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL DEFAULT 0,
  message_count INTEGER NOT NULL, tool_call_count INTEGER NOT NULL, subagent_count INTEGER NOT NULL,
  in_tok INTEGER NOT NULL, out_tok INTEGER NOT NULL, cr_tok INTEGER NOT NULL, cc_tok INTEGER NOT NULL,
  main_in_tok INTEGER NOT NULL, main_out_tok INTEGER NOT NULL, main_cr_tok INTEGER NOT NULL, main_cc_tok INTEGER NOT NULL,
  git_branch TEXT, primary_model TEXT, models_json TEXT NOT NULL DEFAULT '[]', versions_json TEXT NOT NULL DEFAULT '[]',
  leaf_uuid TEXT, root_uuid TEXT, fork_origin_id TEXT, fork_point_uuid TEXT, is_empty INTEGER NOT NULL DEFAULT 0,
  dup_uuids INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0, unpriced INTEGER NOT NULL DEFAULT 0,
  continued_in_id TEXT, superseded INTEGER NOT NULL DEFAULT 0);
CREATE INDEX sessions_project_active ON sessions(project_id, last_active_ms DESC);
CREATE INDEX sessions_root ON sessions(root_uuid);
CREATE INDEX sessions_continued_in ON sessions(continued_in_id) WHERE continued_in_id IS NOT NULL;
CREATE TABLE messages (
  session_id TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT '', key TEXT NOT NULL, role INTEGER NOT NULL,
  ts_ms INTEGER NOT NULL, model TEXT, in_tok INTEGER NOT NULL DEFAULT 0, out_tok INTEGER NOT NULL DEFAULT 0,
  cr_tok INTEGER NOT NULL DEFAULT 0, cc_tok INTEGER NOT NULL DEFAULT 0,
  cc1h_tok INTEGER NOT NULL DEFAULT 0, fast INTEGER NOT NULL DEFAULT 0, logged_cost REAL, cost REAL,
  billed INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (session_id, agent_id, key)) WITHOUT ROWID;
CREATE INDEX messages_ts ON messages(ts_ms);
CREATE INDEX messages_key ON messages(key);
CREATE TABLE tool_calls (session_id TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT '', tool_use_id TEXT NOT NULL,
  name TEXT NOT NULL, ts_ms INTEGER, is_error INTEGER NOT NULL DEFAULT 0, file_path TEXT,
  PRIMARY KEY (session_id, tool_use_id)) WITHOUT ROWID;
CREATE INDEX tool_calls_name ON tool_calls(name);
CREATE TABLE session_projects (session_id TEXT NOT NULL, project_id TEXT NOT NULL,
  PRIMARY KEY (session_id, project_id)) WITHOUT ROWID;
CREATE INDEX session_projects_project ON session_projects(project_id);
CREATE TABLE subagents (session_id TEXT NOT NULL, agent_id TEXT NOT NULL, agent_type TEXT, parent_agent_id TEXT,
  tool_use_id TEXT, workflow_run_id TEXT, started_ms INTEGER, ended_ms INTEGER, out_tok INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, agent_id)) WITHOUT ROWID;
CREATE TABLE persisted_outputs (session_id TEXT NOT NULL, file_name TEXT NOT NULL, tool_use_id TEXT NOT NULL,
  agent_id TEXT, PRIMARY KEY (session_id, file_name)) WITHOUT ROWID;
CREATE TABLE diag_counts (file_id INTEGER NOT NULL, category INTEGER NOT NULL,
  name TEXT NOT NULL, version TEXT NOT NULL DEFAULT '', count INTEGER NOT NULL,
  PRIMARY KEY (file_id, category, name, version)) WITHOUT ROWID;
CREATE TABLE msg_text (id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL, session_id TEXT NOT NULL, agent_id TEXT,
  node_uuid TEXT NOT NULL, block_idx INTEGER NOT NULL, role INTEGER NOT NULL,
  ts_ms INTEGER, on_main_line INTEGER NOT NULL DEFAULT 1, body TEXT NOT NULL, tool_use_id TEXT,
  UNIQUE (session_id, node_uuid, block_idx));
CREATE INDEX msg_text_file ON msg_text(file_id);
CREATE INDEX msg_text_session ON msg_text(session_id);
CREATE VIRTUAL TABLE msg_fts USING fts5(body, content='msg_text', content_rowid='id', tokenize='trigram');
CREATE TRIGGER msg_text_ai AFTER INSERT ON msg_text BEGIN INSERT INTO msg_fts(rowid, body) VALUES (new.id, new.body); END;
CREATE TRIGGER msg_text_ad AFTER DELETE ON msg_text BEGIN INSERT INTO msg_fts(msg_fts, rowid, body) VALUES ('delete', old.id, old.body); END;
"#;

/// Drops every object created by [`DDL`] (used by `rebuild`).
pub const DROP_ALL: &str = r#"
DROP TRIGGER IF EXISTS msg_text_ai;
DROP TRIGGER IF EXISTS msg_text_ad;
DROP TABLE IF EXISTS msg_fts;
DROP TABLE IF EXISTS msg_text;
DROP TABLE IF EXISTS diag_counts;
DROP TABLE IF EXISTS persisted_outputs;
DROP TABLE IF EXISTS subagents;
DROP TABLE IF EXISTS session_projects;
DROP TABLE IF EXISTS tool_calls;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS sessions;
DROP TABLE IF EXISTS projects;
DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS meta;
"#;
