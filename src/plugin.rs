//! The ccrpc Plugin (ADR-0003): generated into ccrpc's data directory as a local marketplace and
//! installed with Claude Code's own CLI (`claude plugin …`), so ccrpc never writes Claude Code's
//! configuration itself. The plugin holds hooks only (no skills, agents, commands or MCP servers:
//! nothing that costs model context); each hook runs `<this ccrpc> hook`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::hook::HOOK_PROTOCOL;
use crate::locate::Roots;

pub const MARKETPLACE: &str = "ccrpc";
pub const PLUGIN: &str = "ccrpc";
pub const PLUGIN_ID: &str = "ccrpc@ccrpc";
/// How long a PermissionRequest hook may wait for the caller's answer.
pub const PERMISSION_TIMEOUT_SECS: u64 = 86_400;

/// Hook events the plugin registers: `(event, matcher)`.
const EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PermissionRequest", Some("*")),
    ("PostToolUse", Some("*")),
    ("PostToolUseFailure", Some("*")),
    ("PermissionDenied", Some("*")),
    ("Stop", None),
    ("StopFailure", None),
];

/// `$XDG_DATA_HOME/ccrpc/plugin`, else `~/.local/share/ccrpc/plugin`: the marketplace root.
pub fn marketplace_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    base.join("ccrpc/plugin")
}

/// The plugin's version: ccrpc's version and the hook protocol (a new one makes Claude Code
/// refresh its cached copy).
pub fn plugin_version() -> String {
    format!("{}-hook{HOOK_PROTOCOL}", env!("CARGO_PKG_VERSION"))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command every hook runs.
pub fn hook_command(exe: &Path) -> String {
    format!("{} hook", shell_quote(&exe.display().to_string()))
}

/// `hooks/hooks.json` for `exe`.
pub fn hooks_json(exe: &Path) -> Value {
    let cmd = hook_command(exe);
    let mut hooks = serde_json::Map::new();
    for (event, matcher) in EVENTS {
        let mut h = json!({"type": "command", "command": cmd});
        if *event == "PermissionRequest" {
            h["timeout"] = json!(PERMISSION_TIMEOUT_SECS);
        }
        let mut entry = json!({"hooks": [h]});
        if let Some(m) = matcher {
            entry["matcher"] = json!(m);
        }
        hooks.insert(event.to_string(), json!([entry]));
    }
    json!({"hooks": hooks})
}

/// Writes the marketplace with the plugin into `root`; returns the plugin's own directory (the one
/// `claude --plugin-dir` takes).
pub fn generate(root: &Path, exe: &Path) -> Result<PathBuf> {
    let plugin = root.join(PLUGIN);
    let write = |p: PathBuf, v: Value| -> Result<()> {
        std::fs::create_dir_all(p.parent().context("没有上级目录")?)?;
        std::fs::write(&p, serde_json::to_string_pretty(&v)? + "\n")
            .with_context(|| format!("写不了 {}", p.display()))
    };
    write(
        root.join(".claude-plugin/marketplace.json"),
        json!({
            "name": MARKETPLACE,
            "owner": {"name": "ccrpc"},
            "plugins": [{"name": PLUGIN, "source": format!("./{PLUGIN}"),
                "description": "ccrpc 的 hooks：只在 clay Pane 里把事件报告给 ccrpc、代答审批与提问"}],
        }),
    )?;
    write(
        plugin.join(".claude-plugin/plugin.json"),
        json!({"name": PLUGIN, "version": plugin_version(), "author": {"name": "ccrpc"},
            "description": "ccrpc 的 hooks：只在 clay Pane 里把事件报告给 ccrpc、代答审批与提问"}),
    )?;
    write(plugin.join("hooks/hooks.json"), hooks_json(exe))?;
    Ok(plugin)
}

fn claude(roots: &Roots, args: &[&str]) -> Result<std::process::Output> {
    let mut c = Command::new("claude");
    c.args(args).env("CLAUDE_CONFIG_DIR", &roots.claude_config);
    c.output().context("运行 claude 失败（claude 在 PATH 里吗？）")
}

fn ok_or_bail(what: &str, out: &std::process::Output) -> Result<()> {
    if out.status.success() {
        return Ok(());
    }
    bail!(
        "{what} 失败：{}{}",
        String::from_utf8_lossy(&out.stderr).trim(),
        String::from_utf8_lossy(&out.stdout).trim()
    )
}

fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("找不到 ccrpc 自己的路径")?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

/// `ccrpc plugin install`: (re)generates the plugin for this ccrpc and installs or updates it as a
/// user-scope plugin. Running Sessions load it only when claude restarts.
pub fn install(roots: &Roots) -> Result<()> {
    let root = marketplace_dir();
    generate(&root, &current_exe()?)?;
    let dir = root.display().to_string();
    let listed = claude(roots, &["plugin", "marketplace", "list"])?;
    if String::from_utf8_lossy(&listed.stdout).contains(&dir) {
        ok_or_bail("更新 marketplace", &claude(roots, &["plugin", "marketplace", "update", MARKETPLACE])?)?;
    } else {
        ok_or_bail("添加 marketplace", &claude(roots, &["plugin", "marketplace", "add", &dir])?)?;
    }
    if installed(roots) {
        ok_or_bail("更新插件", &claude(roots, &["plugin", "update", PLUGIN_ID])?)?;
    } else {
        ok_or_bail("安装插件", &claude(roots, &["plugin", "install", PLUGIN_ID])?)?;
    }
    println!("已安装 {PLUGIN_ID}（{}）：之后启动的 claude 生效，已在运行的要重启", plugin_version());
    Ok(())
}

pub fn uninstall(roots: &Roots) -> Result<()> {
    if installed(roots) {
        ok_or_bail("卸载插件", &claude(roots, &["plugin", "uninstall", PLUGIN_ID])?)?;
    }
    let _ = claude(roots, &["plugin", "marketplace", "remove", MARKETPLACE]);
    println!("已卸载 {PLUGIN_ID}");
    Ok(())
}

/// The plugin is listed by `claude plugin list`.
fn installed(roots: &Roots) -> bool {
    claude(roots, &["plugin", "list"])
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(PLUGIN_ID))
        .unwrap_or(false)
}

/// `enabledPlugins` of the user settings has the plugin on.
pub fn enabled(roots: &Roots) -> bool {
    std::fs::read(roots.claude_config.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|v| v["enabledPlugins"][PLUGIN_ID].as_bool() == Some(true))
}

/// The hook command of the installed copies (Claude Code caches plugins under `plugins/cache`).
pub fn installed_commands(roots: &Roots) -> Vec<String> {
    let base = roots.claude_config.join("plugins/cache").join(MARKETPLACE).join(PLUGIN);
    let Ok(rd) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let v = std::fs::read(e.path().join("hooks/hooks.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if let Some(c) = v.as_ref().and_then(|v| v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str()) {
            out.push(c.to_string());
        }
    }
    out
}

/// One line per finding for `ccrpc doctor`: `(level, text)`.
pub fn doctor(roots: &Roots) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if !enabled(roots) {
        out.push(("fail", format!("ccrpc 插件未启用（{PLUGIN_ID}）：运行 ccrpc plugin install")));
        return out;
    }
    let want = current_exe().map(|e| hook_command(&e)).unwrap_or_default();
    let cmds = installed_commands(roots);
    if cmds.contains(&want) {
        out.push(("ok", format!("ccrpc 插件已启用（{}）", plugin_version())));
    } else {
        out.push((
            "warn",
            format!("ccrpc 插件的 hook 不是调用本 ccrpc（{want}）：重新运行 ccrpc plugin install"),
        ));
    }
    out
}

pub fn cmd(roots: &Roots, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("install") => install(roots),
        Some("uninstall") => uninstall(roots),
        Some("generate") => {
            let dir = args.get(1).context("用法：ccrpc plugin generate <目录>")?;
            let p = generate(Path::new(dir), &current_exe()?)?;
            println!("{}", p.display());
            Ok(())
        }
        _ => bail!("用法：ccrpc plugin install | uninstall | generate <目录>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_run_this_ccrpc_and_only_permission_requests_wait_long() {
        let v = hooks_json(Path::new("/opt/it's here/ccrpc"));
        let stop = &v["hooks"]["Stop"][0];
        assert_eq!(stop["hooks"][0]["command"], r"'/opt/it'\''s here/ccrpc' hook");
        assert!(stop.get("matcher").is_none());
        assert!(stop["hooks"][0].get("timeout").is_none());
        let pr = &v["hooks"]["PermissionRequest"][0];
        assert_eq!(pr["matcher"], "*");
        assert_eq!(pr["hooks"][0]["timeout"], PERMISSION_TIMEOUT_SECS);
        assert_eq!(v["hooks"].as_object().unwrap().len(), EVENTS.len());
    }

    #[test]
    fn generates_a_marketplace_with_the_plugin() {
        let d = tempfile::tempdir().unwrap();
        let p = generate(d.path(), Path::new("/x/ccrpc")).unwrap();
        assert_eq!(p, d.path().join("ccrpc"));
        let m: Value =
            serde_json::from_slice(&std::fs::read(d.path().join(".claude-plugin/marketplace.json")).unwrap()).unwrap();
        assert_eq!(m["plugins"][0]["source"], "./ccrpc");
        assert!(p.join(".claude-plugin/plugin.json").is_file());
        assert!(p.join("hooks/hooks.json").is_file());
    }
}
