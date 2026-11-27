//! The Host's own settings (desktop: `~/Library/Application Support/dev.joy.codeck/settings.json`; the
//! daemon's are in its state directory).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    /// The Projects in the sidebar, in the order added; `None` until the first 「添加 Project」 (the app then
    /// opens that dialog by itself).
    #[serde(default)]
    pub sidebar_projects: Option<Vec<SidebarProject>>,
    /// 「设置 › 通用 › 局域网访问」: the app serves its `remote` methods on the LAN (ADR-0012).
    #[serde(default)]
    pub lan_access: bool,
    /// 「设置 › 通用 › 缓存保活」: how many minutes an idle Live Session's prompt cache goes unused before the Host
    /// refreshes it (`keepalive.rs`); `None` = [`DEFAULT_KEEP_ALIVE_MINUTES`], `Some(0)` = off.
    #[serde(default)]
    pub keep_alive_minutes: Option<u32>,
}

pub const DEFAULT_KEEP_ALIVE_MINUTES: u32 = 40;

impl Settings {
    /// 「缓存保活」 in minutes; 0 = off.
    pub fn keep_alive_minutes(&self) -> u32 {
        self.keep_alive_minutes
            .unwrap_or(DEFAULT_KEEP_ALIVE_MINUTES)
    }
}

/// A Project added to the sidebar: a directory (usually an indexed Project's path). `nest` gathers the Sessions of
/// the Projects under it into its group (「收拢子 Project」).
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SidebarProject {
    pub path: String,
    #[serde(default)]
    pub nest: bool,
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

/// Trims each path (and a trailing `/`), drops empty ones and later repeats.
pub fn normalize_sidebar(projects: Vec<SidebarProject>) -> Vec<SidebarProject> {
    let mut out: Vec<SidebarProject> = Vec::new();
    for p in projects {
        let trimmed = p.path.trim();
        let path = match trimmed.trim_end_matches('/') {
            "" if trimmed.starts_with('/') => "/",
            t => t,
        };
        if path.is_empty() || out.iter().any(|o| o.path == path) {
            continue;
        }
        out.push(SidebarProject {
            path: path.to_owned(),
            nest: p.nest,
        });
    }
    out
}

/// The desktop app's settings file.
pub fn default_settings_path() -> PathBuf {
    codeck_core::paths::default_settings_dir().join(SETTINGS_FILE)
}

/// Set once per process by `Host::start`.
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Makes `path` the settings file of this process; a later call with another path is ignored (one
/// Host per process).
pub fn set_path(path: PathBuf) {
    if PATH.get_or_init(|| path.clone()) != &path {
        tracing::warn!("settings file already set; ignoring {}", path.display());
    }
}

pub fn settings_path() -> PathBuf {
    PATH.get().cloned().unwrap_or_else(default_settings_path)
}

/// Verifies that an existing settings file is a regular file (not a symlink), owned by the
/// current effective UID, and not writable by group or others. A missing file is allowed.
pub fn verify_file_trust(path: &Path) -> Result<(), String> {
    open_trusted(path).map(|_| ())
}

fn open_trusted(path: &Path) -> Result<Option<std::fs::File>, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!("配置文件不能是符号链接（{}）", path.display()));
        }
        Err(e) => return Err(format!("读取配置文件失败（{}）：{e}", path.display())),
    };
    let meta = file
        .metadata()
        .map_err(|e| format!("检查配置文件失败（{}）：{e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("配置文件不是普通文件（{}）", path.display()));
    }
    // SAFETY: `geteuid` has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        return Err(format!("配置文件的属主不是当前用户（{}）", path.display()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if (mode & 0o022) != 0 {
        return Err(format!(
            "配置文件可被组或其他用户写入（当前 {:04o}）：{}",
            mode,
            path.display()
        ));
    }
    Ok(Some(file))
}

/// Missing, untrusted, or unreadable settings fall back to defaults.
pub fn load() -> Settings {
    load_from(&settings_path())
}

pub fn load_from(path: &Path) -> Settings {
    use std::io::Read;
    let mut file = match open_trusted(path) {
        Ok(Some(f)) => f,
        Ok(None) => return Settings::default(),
        Err(why) => {
            tracing::warn!("ignoring untrusted settings file: {why}");
            return Settings::default();
        }
    };
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Settings::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
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
            sidebar_projects: Some(vec![SidebarProject {
                path: "/p".into(),
                nest: true,
            }]),
            lan_access: true,
            keep_alive_minutes: Some(0),
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
    fn sidebar_paths_are_trimmed_and_deduplicated() {
        let p = |path: &str, nest| SidebarProject {
            path: path.into(),
            nest,
        };
        assert_eq!(
            normalize_sidebar(vec![
                p(" /a/ ", true),
                p("", false),
                p("/a", false),
                p("/b", false)
            ]),
            vec![p("/a", true), p("/b", false)]
        );
    }

    #[test]
    fn corrupt_or_missing_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        assert_eq!(load_from(&path), Settings::default());
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_from(&path), Settings::default());
    }

    #[test]
    fn verify_file_trust_and_load_reject_writable_and_symlink_settings() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        verify_file_trust(&path).unwrap();

        let custom = Settings {
            launch_rules: vec![LaunchRule {
                dir: "/repo".into(),
                args: "--model opus".into(),
            }],
            ..Settings::default()
        };
        save_to(&path, &custom).unwrap();
        verify_file_trust(&path).unwrap();
        assert_eq!(load_from(&path), custom);

        // Group-writable or world-writable settings.json must be rejected and ignored by load_from.
        for bad_mode in [0o660, 0o602] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(bad_mode)).unwrap();
            let err = verify_file_trust(&path).unwrap_err();
            assert!(err.contains("组或其他用户写入"), "{bad_mode:04o}: {err}");
            assert_eq!(load_from(&path), Settings::default());
        }

        // Symlinked settings.json must also be rejected.
        let real = dir.path().join("real-settings.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(&path, &real).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        let err = verify_file_trust(&path).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
        assert_eq!(load_from(&path), Settings::default());
    }
}
