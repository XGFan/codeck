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
pub const AGY_ROOT_ENV: &str = "CODECK_AGY_ROOT";
pub const HAA_HOME_ENV: &str = "CODECK_HAA_HOME";
pub const CODEX_ROOT_ENV: &str = "CODECK_CODEX_ROOT";
pub const PI_ROOT_ENV: &str = "CODECK_PI_ROOT";
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

/// Resolves the agy data root: `$CODECK_AGY_ROOT`, else `~/.gemini/antigravity-cli`.
pub fn resolve_agy_root() -> PathBuf {
    resolve_agy_root_with(std::env::var_os(AGY_ROOT_ENV), dirs::home_dir())
}

/// Pure resolution of the agy data root: non-empty `env` (expanding `~/`), else
/// `<home>/.gemini/antigravity-cli`.
pub fn resolve_agy_root_with(env: Option<OsString>, home: Option<PathBuf>) -> PathBuf {
    if let Some(v) = env.filter(|v| !v.is_empty()) {
        return expand_tilde(Path::new(&v), home.as_deref());
    }
    home.unwrap_or_else(|| PathBuf::from("/"))
        .join(".gemini")
        .join("antigravity-cli")
}

/// Resolves the haa home directory: `$CODECK_HAA_HOME`, else `~/.haa`.
pub fn resolve_haa_home() -> PathBuf {
    resolve_haa_home_with(std::env::var_os(HAA_HOME_ENV), dirs::home_dir())
}

/// Pure resolution of the haa home directory: non-empty `env` (expanding `~/`), else `<home>/.haa`.
pub fn resolve_haa_home_with(env: Option<OsString>, home: Option<PathBuf>) -> PathBuf {
    if let Some(v) = env.filter(|v| !v.is_empty()) {
        return expand_tilde(Path::new(&v), home.as_deref());
    }
    home.unwrap_or_else(|| PathBuf::from("/")).join(".haa")
}

/// Resolves codex's data root: `$CODECK_CODEX_ROOT`, else `~/.codex`.
pub fn resolve_codex_root() -> PathBuf {
    resolve_root_with(
        std::env::var_os(CODEX_ROOT_ENV),
        dirs::home_dir(),
        &[".codex"],
    )
}

/// Resolves pi's agent directory: `$CODECK_PI_ROOT`, else `~/.pi/agent`.
pub fn resolve_pi_root() -> PathBuf {
    resolve_root_with(
        std::env::var_os(PI_ROOT_ENV),
        dirs::home_dir(),
        &[".pi", "agent"],
    )
}

/// Pure resolution of a read-only root: non-empty `env` (expanding `~/`), else `<home>/<default…>`.
pub fn resolve_root_with(
    env: Option<OsString>,
    home: Option<PathBuf>,
    default: &[&str],
) -> PathBuf {
    if let Some(v) = env.filter(|v| !v.is_empty()) {
        return expand_tilde(Path::new(&v), home.as_deref());
    }
    default
        .iter()
        .fold(home.unwrap_or_else(|| PathBuf::from("/")), |p, c| p.join(c))
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

/// `<cache dir>/plan-usage`: private working directory for the Host's plan-usage probe.
pub fn plan_usage_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("plan-usage")
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

    #[test]
    fn codex_and_pi_root_resolution() {
        let home = Some(PathBuf::from("/Users/dev"));
        assert_eq!(
            resolve_root_with(None, home.clone(), &[".pi", "agent"]),
            PathBuf::from("/Users/dev/.pi/agent")
        );
        assert_eq!(
            resolve_root_with(Some("".into()), home.clone(), &[".codex"]),
            PathBuf::from("/Users/dev/.codex")
        );
        assert_eq!(
            resolve_root_with(Some("~/c".into()), home, &[".codex"]),
            PathBuf::from("/Users/dev/c")
        );
    }

    #[test]
    fn agy_root_and_haa_home_resolution() {
        let home = Some(PathBuf::from("/Users/dev"));
        assert_eq!(
            resolve_agy_root_with(None, home.clone()),
            PathBuf::from("/Users/dev/.gemini/antigravity-cli")
        );
        assert_eq!(
            resolve_agy_root_with(Some("".into()), home.clone()),
            PathBuf::from("/Users/dev/.gemini/antigravity-cli")
        );
        assert_eq!(
            resolve_agy_root_with(Some("~/custom-agy".into()), home.clone()),
            PathBuf::from("/Users/dev/custom-agy")
        );
        assert_eq!(
            resolve_haa_home_with(None, home.clone()),
            PathBuf::from("/Users/dev/.haa")
        );
        assert_eq!(
            resolve_haa_home_with(Some("".into()), home.clone()),
            PathBuf::from("/Users/dev/.haa")
        );
        assert_eq!(
            resolve_haa_home_with(Some("~/custom-haa".into()), home),
            PathBuf::from("/Users/dev/custom-haa")
        );
    }
}
