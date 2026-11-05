use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NameCount {
    pub name: String,
    pub count: u32,
    /// Claude Code versions the name was seen in.
    pub versions: Vec<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileFailure {
    pub path: String,
    pub failed_lines: u32,
    pub first_error: String,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionStat {
    pub version: String,
    pub sessions: u32,
    pub failed_lines: u32,
    pub unknown_items: u32,
    #[specta(type = Number)]
    pub last_seen_ms: f64,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub files_scanned: u32,
    pub sessions: u32,
    pub empty_sessions: u32,
    pub failed_lines: u32,
    pub files_with_failures: Vec<FileFailure>,
    pub unknown_entry_types: Vec<NameCount>,
    pub unknown_block_types: Vec<NameCount>,
    pub unknown_system_subtypes: Vec<NameCount>,
    pub unknown_tools: Vec<NameCount>,
    pub versions: Vec<VersionStat>,
    pub duplicate_uuids: u32,
    pub orphan_subagents: u32,
}
