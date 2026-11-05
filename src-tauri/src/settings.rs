//! The app's own settings (`~/Library/Application Support/dev.joy.codeck/settings.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Data root override; `None` = `$CLAUDE_CONFIG_DIR` or `~/.claude`.
    pub data_root: Option<String>,
    /// ccrpc's screen analyzer, over the login shell's `CCRPC_ANALYZER_*` (see `ccrpc::env`).
    #[serde(default)]
    pub analyzer: AnalyzerSettings,
    /// Extra claude arguments by directory for Sessions CoDeck starts (see `ccrpc::launch`).
    #[serde(default)]
    pub launch_rules: Vec<LaunchRule>,
    /// 「设置 › ccrpc › 新开的 Pane」: Sessions CoDeck starts (new and Resume) open in the ToH Mac Client's hidden
    /// window (`ccrpc new --hidden`, ToH ADR-0050).
    #[serde(default)]
    pub hide_new_panes: bool,
}

/// 「设置 › ccrpc › 启动参数」: `args` (shell-style words) go to claude when a Session starts in `dir` or below.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRule {
    pub dir: String,
    pub args: String,
}

/// What 「设置 › ccrpc」 sets for ccrpc's analyzer; `None` leaves the login shell's variable (or ccrpc's
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

/// Serializes read-modify-write cycles: the settings file holds unrelated fields (data root,
/// analyzer, launch rules) saved by different commands, which may run at the same time.
static WRITE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Applies `f` to the saved settings and saves the result, which it returns.
pub fn update(f: impl FnOnce(&mut Settings)) -> std::io::Result<Settings> {
    update_at(&settings_path(), f)
}

fn update_at(path: &Path, f: impl FnOnce(&mut Settings)) -> std::io::Result<Settings> {
    let _g = WRITE.lock();
    let mut s = load_from(path);
    f(&mut s);
    save_to(path, &s)?;
    Ok(s)
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
            hide_new_panes: true,
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
    fn concurrent_updates_of_different_fields_are_all_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..50 {
                    update_at(&path, |s| {
                        s.launch_rules.push(LaunchRule {
                            dir: format!("/d{i}"),
                            args: "-x".into(),
                        })
                    })
                    .unwrap();
                }
            });
            scope.spawn(|| {
                for i in 0..50 {
                    update_at(&path, |s| s.analyzer.model = Some(format!("m{i}"))).unwrap();
                }
            });
        });
        let s = load_from(&path);
        assert_eq!(s.launch_rules.len(), 50);
        assert_eq!(s.analyzer.model.as_deref(), Some("m49"));
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
