//! Data root resolution (settings > `$CLAUDE_CONFIG_DIR` > `~/.claude`) and the app's own directories.
//!
//! The data root is read-only for this app (ADR-0002); the cache and settings live in the app's own
//! Caches / Application Support folders (ADR-0003).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::model::DataRootSource;

/// Bundle identifier; also the name of the app's cache and settings folders.
pub const BUNDLE_ID: &str = "dev.joy.codeck";
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
pub const INDEX_FILE_NAME: &str = "index.sqlite";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataRoot {
    pub path: PathBuf,
    pub source: DataRootSource,
}

/// Resolves the data root from the process environment and the user's home directory.
pub fn resolve_data_root(settings_override: Option<&Path>) -> DataRoot {
    resolve_data_root_with(
        settings_override,
        std::env::var_os(CONFIG_DIR_ENV),
        dirs::home_dir(),
    )
}

/// Pure resolution: settings override, then a non-empty `env` value, then `<home>/.claude`.
/// A leading `~/` is expanded against `home`.
pub fn resolve_data_root_with(
    settings_override: Option<&Path>,
    env: Option<OsString>,
    home: Option<PathBuf>,
) -> DataRoot {
    if let Some(p) = settings_override.filter(|p| !p.as_os_str().is_empty()) {
        return DataRoot {
            path: expand_tilde(p, home.as_deref()),
            source: DataRootSource::Settings,
        };
    }
    if let Some(v) = env.filter(|v| !v.is_empty()) {
        return DataRoot {
            path: expand_tilde(Path::new(&v), home.as_deref()),
            source: DataRootSource::Env,
        };
    }
    let home = home.unwrap_or_else(|| PathBuf::from("/"));
    DataRoot {
        path: home.join(".claude"),
        source: DataRootSource::Default,
    }
}

fn expand_tilde(p: &Path, home: Option<&Path>) -> PathBuf {
    match (p.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

/// `<data root>/projects`.
pub fn projects_dir(root: &Path) -> PathBuf {
    root.join("projects")
}

/// `<data root>/sessions` (live process files).
pub fn sessions_dir(root: &Path) -> PathBuf {
    root.join("sessions")
}

/// `~/Library/Caches/dev.joy.codeck`.
pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(BUNDLE_ID)
}

/// `~/Library/Application Support/dev.joy.codeck` (settings.json lives here).
pub fn default_settings_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(BUNDLE_ID)
}

/// `<cache dir>/index.sqlite`.
pub fn index_db_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(INDEX_FILE_NAME)
}

/// `<cache dir>/pricing.json`: `claude-*` entries of the last price list fetched online.
pub fn prices_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("pricing.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_root_priority() {
        let home = Some(PathBuf::from("/Users/dev"));
        let r = resolve_data_root_with(Some(Path::new("/x")), Some("/env".into()), home.clone());
        assert_eq!(
            r,
            DataRoot {
                path: "/x".into(),
                source: DataRootSource::Settings
            }
        );
        let r = resolve_data_root_with(None, Some("~/cfg".into()), home.clone());
        assert_eq!(
            r,
            DataRoot {
                path: "/Users/dev/cfg".into(),
                source: DataRootSource::Env
            }
        );
        let r = resolve_data_root_with(Some(Path::new("")), Some("".into()), home);
        assert_eq!(
            r,
            DataRoot {
                path: "/Users/dev/.claude".into(),
                source: DataRootSource::Default
            }
        );
    }
}
