//! The app's own settings (`~/Library/Application Support/dev.joy.codeck/settings.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Data root override; `None` = `$CLAUDE_CONFIG_DIR` or `~/.claude`.
    pub data_root: Option<String>,
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

/// Writes atomically (temp file + rename) so a crash never leaves a half-written file.
pub fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
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
        };
        save_to(&path, &some).unwrap();
        assert_eq!(load_from(&path), some);
        save_to(&path, &Settings::default()).unwrap();
        assert_eq!(load_from(&path).data_root, None);
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
