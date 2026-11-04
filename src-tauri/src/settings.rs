//! The app's own settings (`~/Library/Application Support/dev.joy.codeck/settings.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Data root override; `None` = `$CLAUDE_CONFIG_DIR` or `~/.claude`.
    pub data_root: Option<String>,
    /// ccrpc's screen analyzer, over the login shell's `CCRPC_ANALYZER_*` (see `drive::env`).
    #[serde(default)]
    pub analyzer: AnalyzerSettings,
    /// Extra claude arguments by directory for Sessions CoDeck starts (see `drive::launch`).
    #[serde(default)]
    pub launch_rules: Vec<LaunchRule>,
}

/// 「设置 › 驾驶 › 启动参数」: `args` (shell-style words) go to claude when a Session starts in `dir` or below.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRule {
    pub dir: String,
    pub args: String,
}

/// What 「设置 › 驾驶」 sets for ccrpc's analyzer; `None` leaves the login shell's variable (or ccrpc's
/// default) in place.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerSettings {
    /// `CCRPC_ANALYZER_API_KEY`.
    pub api_key: Option<String>,
    /// `CCRPC_ANALYZER_BASE_URL` (an Anthropic Messages endpoint).
    pub base_url: Option<String>,
    /// `CCRPC_ANALYZER_MODEL`.
    pub model: Option<String>,
}

pub fn settings_path() -> PathBuf {
    codeck_core::paths::default_settings_dir().join(SETTINGS_FILE)
}

/// Missing or unreadable settings fall back to defaults.
pub fn load() -> Settings {
    load_from(&settings_path())
}

pub fn load_from(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    save_to(&settings_path(), settings)
}

/// Writes atomically (temp file + rename) so a crash never leaves a half-written file. Only the user
/// may read it: it can hold the analyzer's API key.
pub fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(settings)?)?;
    drop(f);
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_data_root() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(SETTINGS_FILE);
        let some = Settings {
            data_root: Some("/data/root".into()),
            analyzer: AnalyzerSettings {
                api_key: Some("k-1".into()),
                base_url: None,
                model: Some("m".into()),
            },
            launch_rules: vec![LaunchRule {
                dir: "/a".into(),
                args: "--add-dir=/b".into(),
            }],
        };
        save_to(&path, &some).unwrap();
        assert_eq!(load_from(&path), some);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        save_to(&path, &Settings::default()).unwrap();
        assert_eq!(load_from(&path).data_root, None);
        // A settings file from before the analyzer fields still loads.
        std::fs::write(&path, br#"{"dataRoot":"/x"}"#).unwrap();
        assert_eq!(load_from(&path).data_root.as_deref(), Some("/x"));
    }

    #[test]
    fn corrupt_or_missing_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        assert_eq!(load_from(&path), Settings::default());
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_from(&path), Settings::default());
    }
}
