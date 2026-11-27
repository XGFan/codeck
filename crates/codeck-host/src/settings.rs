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
    /// 「设置 › 通用」 的通知（`attention`，ADR-0019）.
    #[serde(default)]
    pub notify: NotifyFile,
}

pub const DEFAULT_KEEP_ALIVE_MINUTES: u32 = 40;
/// 「等待作答通知」「已完成通知」 unless set.
pub const DEFAULT_NOTIFY_MINUTES: u32 = 3;
/// 「补推到 iPhone」 unless set.
pub const DEFAULT_ESCALATE_MINUTES: u32 = 5;

impl Settings {
    /// 「缓存保活」 in minutes; 0 = off.
    pub fn keep_alive_minutes(&self) -> u32 {
        self.keep_alive_minutes
            .unwrap_or(DEFAULT_KEEP_ALIVE_MINUTES)
    }
}

/// The notifications' settings as settings.json keeps them: each delay a number of minutes, `"off"`, or not written
/// (the default).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotifyFile {
    #[serde(
        default,
        deserialize_with = "delay",
        skip_serializing_if = "Option::is_none"
    )]
    pub waiting_minutes: Option<Delay>,
    #[serde(
        default,
        deserialize_with = "delay",
        skip_serializing_if = "Option::is_none"
    )]
    pub done_minutes: Option<Delay>,
    #[serde(
        default,
        deserialize_with = "delay",
        skip_serializing_if = "Option::is_none"
    )]
    pub escalate_minutes: Option<Delay>,
    #[serde(default)]
    pub bark: BarkFile,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum Delay {
    Minutes(u32),
    Off(Off),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Off {
    Off,
}

/// A value that is neither a number of minutes nor `"off"` is as if not written: one bad entry must not lose the
/// whole file to the defaults.
fn delay<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Delay>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// Where the iPhone's pushes go (Bark); no device key = none.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BarkFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_key: Option<String>,
}

/// 「设置 › 通用」 的通知 as the window reads and saves them: each delay in minutes, `null` = off.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotifySettings {
    /// 「等待作答通知」 (0 = at once).
    pub waiting_minutes: Option<u32>,
    /// 「已完成通知」 (0 = at once).
    pub done_minutes: Option<u32>,
    /// 「补推到 iPhone」: one only on the Mac goes to the iPhone too after this long unsettled; `null`: only once
    /// nobody uses the Mac.
    pub escalate_minutes: Option<u32>,
    pub bark_server: String,
    /// Empty: no pushes to the iPhone.
    pub bark_device_key: String,
}

impl NotifyFile {
    /// The settings with the defaults filled in.
    pub fn resolved(&self) -> NotifySettings {
        let minutes = |d: Option<Delay>, default: u32| match d {
            None => Some(default),
            Some(Delay::Minutes(m)) => Some(m),
            Some(Delay::Off(_)) => None,
        };
        NotifySettings {
            waiting_minutes: minutes(self.waiting_minutes, DEFAULT_NOTIFY_MINUTES),
            done_minutes: minutes(self.done_minutes, DEFAULT_NOTIFY_MINUTES),
            escalate_minutes: minutes(self.escalate_minutes, DEFAULT_ESCALATE_MINUTES),
            bark_server: crate::attention::bark::server_or_default(self.bark.server.as_deref()),
            bark_device_key: self
                .bark
                .device_key
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string(),
        }
    }

    /// What `s` saves: every delay written out, the server and device key trimmed (blank: not written).
    pub fn from_settings(s: &NotifySettings) -> NotifyFile {
        let delay = |m: Option<u32>| Some(m.map_or(Delay::Off(Off::Off), Delay::Minutes));
        let server = crate::attention::bark::server_or_default(Some(&s.bark_server));
        let key = s.bark_device_key.trim();
        NotifyFile {
            waiting_minutes: delay(s.waiting_minutes),
            done_minutes: delay(s.done_minutes),
            escalate_minutes: delay(s.escalate_minutes),
            bark: BarkFile {
                server: (server != crate::attention::bark::DEFAULT_SERVER).then_some(server),
                device_key: (!key.is_empty()).then(|| key.to_string()),
            },
        }
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

/// Missing or unreadable settings fall back to defaults.
pub fn load() -> Settings {
    load_from(&settings_path())
}

pub fn load_from(path: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(path) else {
        return Settings::default();
    };
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
            notify: NotifyFile {
                waiting_minutes: Some(Delay::Off(Off::Off)),
                done_minutes: Some(Delay::Minutes(0)),
                escalate_minutes: None,
                bark: BarkFile {
                    server: None,
                    device_key: Some("K".into()),
                },
            },
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
    fn notify_delays_are_minutes_off_or_the_default() {
        let file: Settings = serde_json::from_str(
            r#"{"notify":{"waitingMinutes":"off","doneMinutes":0,"escalateMinutes":"soon","bark":{"deviceKey":" K "}}}"#,
        )
        .unwrap();
        let s = file.notify.resolved();
        assert_eq!(
            s,
            NotifySettings {
                waiting_minutes: None,
                done_minutes: Some(0),
                escalate_minutes: Some(DEFAULT_ESCALATE_MINUTES),
                bark_server: crate::attention::bark::DEFAULT_SERVER.into(),
                bark_device_key: "K".into(),
            }
        );
        assert_eq!(
            Settings::default().notify.resolved().waiting_minutes,
            Some(3)
        );
        // Saved: every delay written, the default server and a blank key left out.
        let saved = NotifyFile::from_settings(&NotifySettings {
            bark_device_key: " ".into(),
            ..s.clone()
        });
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::json!({"waitingMinutes":"off","doneMinutes":0,"escalateMinutes":5,"bark":{}})
        );
        assert_eq!(NotifyFile::from_settings(&s).resolved(), s);
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
