//! Layouts: how to recognize a dialog that waits for an answer, whether it is a permission
//! request or a choice, and what each option means. Built-in layouts come from probes and live in
//! the repository's `layouts/`; learned ones come from the analyzer and live in the local layout
//! library (ADR-0002). A layout is only used on a screen that passes the structural checks below.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::recognize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// A tool permission dialog: answered with `answer_permission`.
    Permission,
    /// Any other numbered menu: answered with `answer_choice`.
    Choice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Meaning {
    Allow,
    AllowAlways,
    Deny,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutOption {
    pub number: u32,
    /// The stable start of the option's text (variable parts such as commands or paths left out).
    pub label: String,
    pub meaning: Meaning,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub id: String,
    pub kind: LayoutKind,
    /// The question line of the dialog, trimmed, exactly as shown.
    pub title: String,
    pub options: Vec<LayoutOption>,
    /// `builtin` or `learned` (set when loaded).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at: Option<String>,
    /// The screen it was learned from (a test fixture once exported).
    #[serde(default)]
    pub screen: Vec<String>,
}

/// An option as found on the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenOption {
    pub number: u32,
    /// Full text as shown.
    pub label: String,
    pub meaning: Meaning,
    pub description: Option<String>,
}

impl Layout {
    /// Stable id from the title and option labels.
    pub fn compute_id(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |s: &str| {
            for b in s.bytes().chain([0u8]) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        feed(&self.title);
        for o in &self.options {
            feed(&o.label);
        }
        format!("{:016x}", h)
    }

    /// The structural checks a layout must pass on `lines` (and must pass on its own screen to
    /// be saved): its title line, then a strict numbered menu whose numbers are the layout's and
    /// whose texts start with the layout's labels; a permission layout must offer allow and deny.
    pub fn match_screen(&self, lines: &[String]) -> Option<Vec<ScreenOption>> {
        if self.title.trim().is_empty() || self.options.len() < 2 {
            return None;
        }
        if self.kind == LayoutKind::Permission
            && !(self.options.iter().any(|o| o.meaning == Meaning::Allow)
                && self.options.iter().any(|o| o.meaning == Meaning::Deny))
        {
            return None;
        }
        let q = lines.iter().rposition(|l| l.trim() == self.title.trim())?;
        let start = recognize::last_menu_start(lines, q)?;
        let menu = recognize::strict_menu(lines, start)?;
        if menu.len() != self.options.len() {
            return None;
        }
        let mut out = Vec::new();
        for ((n, text), o) in menu.into_iter().zip(&self.options) {
            if n != o.number || o.label.trim().is_empty() || !text.starts_with(o.label.trim()) {
                return None;
            }
            out.push(ScreenOption {
                number: n,
                label: text,
                meaning: o.meaning,
                description: o.description.clone(),
            });
        }
        Some(out)
    }

    /// Rows from the frame above the title to the menu (for matching a pending tool).
    pub fn body(&self, lines: &[String]) -> Option<String> {
        let q = lines.iter().rposition(|l| l.trim() == self.title.trim())?;
        let start = recognize::last_menu_start(lines, q)?;
        let top = lines[..q]
            .iter()
            .rposition(|l| l.trim_start().starts_with('─') && l.trim().chars().all(|c| c == '─'))
            .map_or(0, |t| t + 1);
        Some(
            lines[top..start]
                .iter()
                .flat_map(|l| l.chars())
                .filter(|c| !c.is_whitespace() && *c != '│')
                .collect(),
        )
    }
}

/// Built-in layouts (`layouts/*.json`, embedded at build time).
fn builtin() -> Vec<Layout> {
    let list: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin_layouts.rs"));
    list.iter()
        .filter_map(|(name, json)| match serde_json::from_str::<Layout>(json) {
            Ok(mut l) => {
                l.source = "builtin".into();
                if l.id.is_empty() {
                    l.id = l.compute_id();
                }
                Some(l)
            }
            Err(e) => {
                eprintln!("ccrpc: 内置布局 {name} 读不出：{e}");
                None
            }
        })
        .collect()
}

/// Directory of the local layout library: `CCRPC_LAYOUT_DIR`, else
/// `$XDG_DATA_HOME/ccrpc/layouts`, else `~/.local/share/ccrpc/layouts`.
pub fn library_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CCRPC_LAYOUT_DIR") {
        return PathBuf::from(d);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("ccrpc").join("layouts")
}

pub struct Library {
    pub layouts: Vec<Layout>,
    dir: PathBuf,
}

impl Library {
    /// Built-in layouts (unless `CCRPC_BUILTIN_LAYOUTS=0`) and the learned ones in `dir`.
    pub fn load(dir: PathBuf) -> Library {
        let mut layouts = if std::env::var("CCRPC_BUILTIN_LAYOUTS").as_deref() == Ok("0") {
            Vec::new()
        } else {
            builtin()
        };
        layouts.extend(read_dir_layouts(&dir));
        Library { layouts, dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The first layout whose structural checks pass on `lines`.
    pub fn find(&self, lines: &[String]) -> Option<(&Layout, Vec<ScreenOption>)> {
        self.layouts
            .iter()
            .find_map(|l| l.match_screen(lines).map(|o| (l, o)))
    }

    /// Stores a learned layout (one file per layout) and uses it from now on.
    pub fn save(&mut self, mut layout: Layout) -> Result<PathBuf> {
        layout.source = String::new();
        if layout.id.is_empty() {
            layout.id = layout.compute_id();
        }
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("建不了布局库目录 {}", self.dir.display()))?;
        let path = self.dir.join(format!("{}.json", layout.id));
        let json = serde_json::to_string_pretty(&layout)?;
        std::fs::write(&path, json + "\n")
            .with_context(|| format!("写不了布局 {}", path.display()))?;
        layout.source = "learned".into();
        self.layouts.retain(|l| l.id != layout.id);
        self.layouts.push(layout);
        Ok(path)
    }
}

/// Learned layouts in `dir` (files that do not parse are skipped).
pub fn read_dir_layouts(dir: &Path) -> Vec<Layout> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let mut l: Layout = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            l.source = "learned".into();
            if l.id.is_empty() {
                l.id = l.compute_id();
            }
            Some(l)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn plan() -> Layout {
        Layout {
            id: String::new(),
            kind: LayoutKind::Choice,
            title: "Claude has written up a plan and is ready to execute. Would you like to proceed?"
                .into(),
            options: vec![
                LayoutOption {
                    number: 1,
                    label: "Yes, and use auto mode".into(),
                    meaning: Meaning::Allow,
                    description: None,
                },
                LayoutOption {
                    number: 2,
                    label: "Yes, manually approve edits".into(),
                    meaning: Meaning::Allow,
                    description: None,
                },
                LayoutOption {
                    number: 3,
                    label: "Tell Claude what to change".into(),
                    meaning: Meaning::Other,
                    description: None,
                },
            ],
            source: String::new(),
            claude_version: None,
            learned_at: None,
            screen: vec![],
        }
    }

    fn plan_screen() -> Vec<String> {
        lines(&[
            "  ────────────────",
            "   Claude has written up a plan and is ready to execute. Would you like to proceed?",
            "",
            "   ❯ 1. Yes, and use auto mode",
            "     2. Yes, manually approve edits",
            "     3. Tell Claude what to change",
            "        shift+tab to approve with this feedback",
            "",
            "   ctrl+g to edit in Vim",
        ])
    }

    #[test]
    fn matches_its_screen_and_nothing_else() {
        let opts = plan().match_screen(&plan_screen()).unwrap();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[2].label, "Tell Claude what to change shift+tab to approve with this feedback");
        // A different menu under the same title is refused.
        let mut other = plan_screen();
        other[4] = "     2. Something else".into();
        assert!(plan().match_screen(&other).is_none());
        // A permission layout must offer allow and deny.
        let mut p = plan();
        p.kind = LayoutKind::Permission;
        assert!(p.match_screen(&plan_screen()).is_none());
    }

    #[test]
    fn saved_layouts_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut lib = Library {
            layouts: vec![],
            dir: dir.path().to_path_buf(),
        };
        let path = lib.save(plan()).unwrap();
        assert!(path.exists());
        let back = read_dir_layouts(dir.path());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source, "learned");
        assert!(lib.find(&plan_screen()).is_some());
    }

    #[test]
    fn builtin_layouts_match_their_own_screens() {
        for l in builtin() {
            assert!(
                l.match_screen(&l.screen).is_some(),
                "内置布局 {} 认不出自己的屏幕",
                l.id
            );
        }
    }
}
