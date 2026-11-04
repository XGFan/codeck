//! Slash commands a Connected Session can run, for completion: Claude Code's built-in ones (a static
//! list), the user's and the project's commands and skills, and those of enabled plugins. Read-only
//! (ADR-0002): files are only opened; whatever is missing or unreadable is skipped.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::model::{SlashCommand, SlashSource};

/// Built-in commands (and bundled skills) of Claude Code 2.1.31x: name, description, whether it
/// opens a panel or picker.
const BUILTIN: &[(&str, &str, bool)] = &[
    ("add-dir", "添加工作目录", false),
    ("agents", "管理 Subagent", true),
    ("clear", "清空对话，开始新的 Session", false),
    ("code-review", "审查当前改动", false),
    ("compact", "压缩对话上下文（可附压缩说明）", false),
    ("config", "设置", true),
    ("context", "查看上下文用量", false),
    ("cost", "查看用量与花费", true),
    ("doctor", "检查安装与配置", true),
    ("exit", "退出 Claude Code", false),
    ("export", "导出对话", true),
    (
        "fewer-permission-prompts",
        "按历史调用补充权限白名单",
        false,
    ),
    ("help", "帮助与命令列表", true),
    ("hooks", "管理 hooks", true),
    ("ide", "连接 IDE", true),
    ("init", "生成 CLAUDE.md", false),
    ("login", "登录", true),
    ("logout", "退出登录", true),
    ("loop", "按间隔重复运行提示词或命令", false),
    ("mcp", "管理 MCP 服务器", true),
    ("memory", "编辑记忆文件", true),
    ("model", "切换模型（带模型名时直接切换）", true),
    ("output-style", "切换输出风格", true),
    ("permissions", "管理工具权限", true),
    ("plugin", "管理插件", true),
    ("pr-comments", "读取 PR 评论", false),
    ("release-notes", "查看更新说明", false),
    ("rename", "重命名 Session", false),
    ("resume", "恢复其他 Session", true),
    ("review", "审查 PR", false),
    ("rewind", "回退对话与代码", true),
    ("schedule", "创建定时运行的云端 Agent", false),
    ("security-review", "对待提交的改动做安全审查", false),
    ("simplify", "简化刚改动的代码", false),
    ("status", "查看版本、模型与账号状态", true),
    ("statusline", "设置状态栏", false),
    ("todos", "查看待办列表", false),
    ("usage", "查看套餐用量", true),
    ("vim", "切换 Vim 编辑模式", false),
];

/// Only this much of a command or skill file is read (the frontmatter and first line are at the top).
const HEAD_BYTES: u64 = 16 * 1024;
/// Descriptions are cut to this many characters.
const DESCRIPTION_CHARS: usize = 200;
/// At most this many files per directory tree (a stray huge folder must not stall completion).
const MAX_FILES: usize = 2000;
/// Command folders nest at most this deep (a symlink loop must end).
const MAX_DEPTH: usize = 6;

/// Every command for a Session started in `cwd` (none of the project's when unknown): built-in
/// first, then the project's, the user's and the plugins'; a name seen earlier hides later ones.
pub fn slash_commands(data_root: &Path, cwd: Option<&Path>) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = BUILTIN
        .iter()
        .map(|&(name, d, interactive)| SlashCommand {
            name: name.to_owned(),
            description: Some(d.to_owned()),
            source: SlashSource::Builtin,
            interactive,
        })
        .collect();
    if let Some(cwd) = cwd {
        let dot = cwd.join(".claude");
        collect_commands(&dot.join("commands"), None, SlashSource::Project, &mut out);
        collect_skills(&dot.join("skills"), None, SlashSource::Project, &mut out);
    }
    collect_commands(
        &data_root.join("commands"),
        None,
        SlashSource::User,
        &mut out,
    );
    collect_skills(&data_root.join("skills"), None, SlashSource::User, &mut out);
    for p in enabled_plugins(data_root, cwd) {
        let ns = Some(p.name.as_str());
        collect_commands(&p.root.join("commands"), ns, SlashSource::Plugin, &mut out);
        for extra in &p.commands {
            collect_commands(extra, ns, SlashSource::Plugin, &mut out);
        }
        collect_skills(&p.root.join("skills"), ns, SlashSource::Plugin, &mut out);
        for extra in &p.skills {
            collect_skills(extra, ns, SlashSource::Plugin, &mut out);
        }
    }
    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out
}

fn push(
    out: &mut Vec<SlashCommand>,
    ns: Option<&str>,
    name: &str,
    description: Option<String>,
    source: SlashSource,
) {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return;
    }
    out.push(SlashCommand {
        name: ns.map_or_else(|| name.to_owned(), |ns| format!("{ns}:{name}")),
        description,
        source,
        interactive: false,
    });
}

/// `*.md` under `dir` (subdirectories only group them; the name is the file's stem), or `dir`
/// itself when it is a command file (a plugin manifest may list files).
fn collect_commands(
    dir: &Path,
    ns: Option<&str>,
    source: SlashSource,
    out: &mut Vec<SlashCommand>,
) {
    let mut files = Vec::new();
    if dir.is_file() {
        files.push(dir.to_path_buf());
    } else {
        md_files(dir, 0, &mut files);
    }
    for f in files {
        let Some(stem) = f.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(text) = read_head(&f) else {
            continue;
        };
        let (fm, body) = frontmatter(&text);
        let description = fm.get("description").cloned().or_else(|| first_line(body));
        push(out, ns, stem, description.map(cut), source);
    }
}

fn md_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        if p.is_dir() {
            md_files(&p, depth + 1, out);
        } else if p.extension().is_some_and(|e| e == "md") {
            out.push(p);
        }
    }
}

/// Skills: `dir/<skill>/SKILL.md`, or `dir/SKILL.md` when `dir` is one skill (a plugin manifest
/// may list skill folders). A skill with `user-invocable: false` is not a command.
fn collect_skills(dir: &Path, ns: Option<&str>, source: SlashSource, out: &mut Vec<SlashCommand>) {
    let mut skills = Vec::new();
    if dir.join("SKILL.md").is_file() {
        skills.push(dir.to_path_buf());
    } else if let Ok(rd) = fs::read_dir(dir) {
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("SKILL.md").is_file())
            .collect();
        dirs.sort();
        skills.extend(dirs.into_iter().take(MAX_FILES));
    }
    for d in skills {
        let Some(text) = read_head(&d.join("SKILL.md")) else {
            continue;
        };
        let (fm, _) = frontmatter(&text);
        if fm.get("user-invocable").is_some_and(|v| v == "false") {
            continue;
        }
        let dir_name = d.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let name = fm.get("name").map_or(dir_name, String::as_str);
        push(
            out,
            ns,
            name,
            fm.get("description").cloned().map(cut),
            source,
        );
    }
}

struct Plugin {
    name: String,
    root: PathBuf,
    commands: Vec<PathBuf>,
    skills: Vec<PathBuf>,
}

/// Installed plugins enabled in the user's settings, overridden by the project's
/// (`.claude/settings.json`, then `.claude/settings.local.json`). A plugin installed for a
/// project counts only in that project.
fn enabled_plugins(data_root: &Path, cwd: Option<&Path>) -> Vec<Plugin> {
    let mut enabled: HashMap<String, bool> = HashMap::new();
    let mut settings = vec![data_root.join("settings.json")];
    if let Some(cwd) = cwd {
        settings.push(cwd.join(".claude/settings.json"));
        settings.push(cwd.join(".claude/settings.local.json"));
    }
    for f in settings {
        if let Some(map) =
            read_json(&f).and_then(|v| v.get("enabledPlugins").and_then(|m| m.as_object()).cloned())
        {
            for (k, v) in map {
                if let Some(b) = v.as_bool() {
                    enabled.insert(k, b);
                }
            }
        }
    }
    let Some(installed) = read_json(&data_root.join("plugins/installed_plugins.json")) else {
        return Vec::new();
    };
    let Some(plugins) = installed.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = plugins.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    for key in keys {
        if enabled.get(key) != Some(&true) {
            continue;
        }
        let installs = plugins[key].as_array().cloned().unwrap_or_default();
        let install = installs.iter().find(|i| {
            let scope = i.get("scope").and_then(|s| s.as_str()).unwrap_or("user");
            let project = i.get("projectPath").and_then(|s| s.as_str());
            scope == "user" || project.is_some_and(|p| cwd.is_some_and(|c| Path::new(p) == c))
        });
        let Some(root) = install
            .and_then(|i| i.get("installPath"))
            .and_then(|p| p.as_str())
            .map(PathBuf::from)
        else {
            continue;
        };
        let manifest = read_json(&root.join(".claude-plugin/plugin.json"));
        let name = manifest
            .as_ref()
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| key.split('@').next().unwrap_or(key).to_owned());
        let paths = |field: &str| -> Vec<PathBuf> {
            let v = manifest.as_ref().and_then(|m| m.get(field));
            let list: Vec<&str> = match v {
                Some(serde_json::Value::String(s)) => vec![s.as_str()],
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect(),
                _ => Vec::new(),
            };
            list.into_iter()
                .map(|p| root.join(p))
                .filter(|p| p.starts_with(&root))
                .collect()
        };
        out.push(Plugin {
            commands: paths("commands"),
            skills: paths("skills"),
            name,
            root,
        });
    }
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let f = fs::File::open(path).ok()?;
    serde_json::from_reader(std::io::BufReader::new(f)).ok()
}

fn read_head(path: &Path) -> Option<String> {
    let mut buf = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The `---` frontmatter's top-level `key: value` pairs (quotes removed; a `>` / `|` block joined
/// into one line) and the body after it. Only what completion needs, not YAML.
fn frontmatter(text: &str) -> (HashMap<String, String>, &str) {
    let mut map = HashMap::new();
    let rest = text.trim_start_matches('\u{feff}');
    let Some(after) = rest
        .strip_prefix("---")
        .filter(|r| r.starts_with(['\n', '\r']))
    else {
        return (map, text);
    };
    let Some(end) = after.find("\n---") else {
        return (map, text);
    };
    let head = &after[..end];
    let body = after[end + 4..].split_once('\n').map_or("", |(_, b)| b);
    let lines: Vec<&str> = head.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let mut v = v.trim().to_owned();
        if matches!(v.as_str(), ">" | "|" | ">-" | "|-" | ">+" | "|+") {
            let mut parts = Vec::new();
            while i < lines.len()
                && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty())
            {
                parts.push(lines[i].trim());
                i += 1;
            }
            v = parts
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
        } else if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            let q = v.chars().next().unwrap();
            v = v[1..v.len() - 1].to_owned();
            v = if q == '"' {
                v.replace("\\\"", "\"").replace("\\\\", "\\")
            } else {
                v.replace("''", "'")
            };
        }
        if !v.is_empty() {
            map.insert(k.trim().to_owned(), v);
        }
    }
    (map, body)
}

/// A command file without a description: its first line of text (a heading's `#` dropped).
fn first_line(body: &str) -> Option<String> {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

fn cut(s: String) -> String {
    if s.chars().count() <= DESCRIPTION_CHARS {
        return s;
    }
    let mut t: String = s.chars().take(DESCRIPTION_CHARS).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn find<'a>(list: &'a [SlashCommand], name: &str) -> Option<&'a SlashCommand> {
        list.iter().find(|c| c.name == name)
    }

    #[test]
    fn collects_user_project_and_enabled_plugin_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("claude");
        let cwd = tmp.path().join("work/app");
        let plugin = root.join("plugins/cache/m/kit/1.0.0");
        let off = root.join("plugins/cache/m/off/1.0.0");
        let other = root.join("plugins/cache/m/elsewhere/1.0.0");

        write(
            &root.join("commands/ship.md"),
            "---\ndescription: \"Ship it: build, tag\"\n---\nbody\n",
        );
        write(
            &root.join("commands/group/note.md"),
            "# Write a note\n\nmore\n",
        );
        write(
            &root.join("skills/tidy/SKILL.md"),
            "---\nname: tidy\ndescription: >\n  Tidy the\n  workspace\n---\n",
        );
        write(
            &root.join("skills/hidden/SKILL.md"),
            "---\nname: hidden\nuser-invocable: false\n---\n",
        );
        // The project's command of the same name wins over the user's.
        write(
            &cwd.join(".claude/commands/ship.md"),
            "---\ndescription: project ship\n---\n",
        );
        write(
            &plugin.join(".claude-plugin/plugin.json"),
            r#"{"name": "kit", "skills": ["./extra/solo/"]}"#,
        );
        write(
            &plugin.join("commands/go.md"),
            "---\ndescription: 'it''s go'\n---\n",
        );
        write(
            &plugin.join("skills/lint/SKILL.md"),
            "---\nname: lint\ndescription: Lint\n---\n",
        );
        write(
            &plugin.join("extra/solo/SKILL.md"),
            "---\nname: solo\n---\n",
        );
        write(&off.join("commands/nope.md"), "x\n");
        write(&other.join("commands/there.md"), "x\n");
        write(
            &root.join("settings.json"),
            r#"{"enabledPlugins": {"kit@m": true, "off@m": true, "elsewhere@m": true}}"#,
        );
        // The project turns one plugin off.
        write(
            &cwd.join(".claude/settings.local.json"),
            r#"{"enabledPlugins": {"off@m": false}}"#,
        );
        let installed = serde_json::json!({"version": 2, "plugins": {
            "kit@m": [{"scope": "user", "installPath": plugin}],
            "off@m": [{"scope": "user", "installPath": off}],
            "elsewhere@m": [{"scope": "project", "projectPath": "/other/project", "installPath": other}],
        }});
        write(
            &root.join("plugins/installed_plugins.json"),
            &installed.to_string(),
        );

        let list = slash_commands(&root, Some(&cwd));
        let desc = |n: &str| find(&list, n).and_then(|c| c.description.clone());
        assert_eq!(
            find(&list, "ship").map(|c| c.source),
            Some(SlashSource::Project)
        );
        assert_eq!(desc("ship").as_deref(), Some("project ship"));
        assert_eq!(desc("note").as_deref(), Some("Write a note"));
        assert_eq!(desc("tidy").as_deref(), Some("Tidy the workspace"));
        assert!(find(&list, "hidden").is_none());
        assert_eq!(desc("kit:go").as_deref(), Some("it's go"));
        assert_eq!(
            find(&list, "kit:lint").map(|c| c.source),
            Some(SlashSource::Plugin)
        );
        assert!(find(&list, "kit:solo").is_some());
        assert!(find(&list, "off:nope").is_none());
        assert!(find(&list, "elsewhere:there").is_none());
        let model = find(&list, "model").unwrap();
        assert_eq!(
            (model.source, model.interactive),
            (SlashSource::Builtin, true)
        );
        assert_eq!(list.iter().filter(|c| c.name == "ship").count(), 1);

        // Without a cwd: no project commands; the user's `ship` shows.
        let list = slash_commands(&root, None);
        assert_eq!(
            find(&list, "ship").map(|c| c.source),
            Some(SlashSource::User)
        );
        assert!(find(&list, "off:nope").is_some());
    }

    #[test]
    fn a_symlink_loop_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("claude");
        write(&root.join("commands/a/x.md"), "x\n");
        std::os::unix::fs::symlink(root.join("commands"), root.join("commands/a/loop")).unwrap();
        let list = slash_commands(&root, None);
        assert!(find(&list, "x").is_some());
    }

    #[test]
    fn missing_data_root_gives_the_built_in_commands() {
        let list = slash_commands(
            Path::new("/nonexistent/claude"),
            Some(Path::new("/nonexistent/cwd")),
        );
        assert_eq!(list.len(), BUILTIN.len());
    }
}
