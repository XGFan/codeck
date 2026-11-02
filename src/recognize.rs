//! Screen recognizers for the interactive UIs ccrpc answers (layouts of Claude Code 2.1.316, see
//! docs/probes.md). Each one returns `None` when the screen does not match what it knows; callers
//! then report `unrecognized` instead of pressing keys.

/// One numbered menu row (`❯ 2. [✔] label`), with wrapped continuation lines joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOption {
    pub number: u32,
    pub label: String,
    /// The `❯` cursor is on this row.
    pub cursor: bool,
    /// Multi-select checkbox state, `None` when the row has no checkbox.
    pub checked: Option<bool>,
    /// Screen row of the option.
    pub row: usize,
}

fn parse_option(line: &str) -> Option<(u32, bool, String)> {
    let t = line.trim_start();
    let (cursor, t) = match t.strip_prefix('❯') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, t),
    };
    let dot = t.find(". ")?;
    let num: u32 = t[..dot].parse().ok()?;
    if num == 0 || num > 20 {
        return None;
    }
    Some((num, cursor, t[dot + 2..].trim_end().to_string()))
}

/// Numbered rows of the bottom-most menu: consecutive numbers 1..n, each optionally followed by
/// indented continuation lines (wrapped label or description).
pub fn menu(lines: &[String]) -> Vec<MenuOption> {
    // Find the last row numbered 1, then collect 1, 2, 3… after it.
    let Some(start) = lines
        .iter()
        .rposition(|l| matches!(parse_option(l), Some((1, _, _))))
    else {
        return Vec::new();
    };
    let mut out: Vec<MenuOption> = Vec::new();
    let mut expect = 1;
    for (row, line) in lines.iter().enumerate().skip(start) {
        if let Some((n, cursor, label)) = parse_option(line)
            && n == expect
        {
            let (checked, label) = checkbox(&label);
            out.push(MenuOption {
                number: n,
                label,
                cursor,
                checked,
                row,
            });
            expect += 1;
        }
    }
    out
}

fn checkbox(label: &str) -> (Option<bool>, String) {
    if let Some(rest) = label.strip_prefix("[ ] ") {
        (Some(false), rest.to_string())
    } else if let Some(rest) = label
        .strip_prefix("[✔] ")
        .or_else(|| label.strip_prefix("[x] "))
    {
        (Some(true), rest.to_string())
    } else {
        (None, label.to_string())
    }
}

/// Wrapped label: the option row plus following lines indented deeper than the number, up to the
/// next option. Used for permission options whose label wraps (long paths).
fn full_label(lines: &[String], opt: &MenuOption, next_row: Option<usize>) -> String {
    let mut s = opt.label.clone();
    let end = next_row.unwrap_or(lines.len());
    for l in &lines[opt.row + 1..end.min(lines.len())] {
        let t = l.trim();
        if t.is_empty() || t.starts_with("Esc to") || t.starts_with('─') {
            break;
        }
        s.push(' ');
        s.push_str(t);
    }
    s
}

/// What a permission option means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Allow,
    AllowAlways,
    Deny,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDialog {
    /// `(number, full label, meaning)`.
    pub options: Vec<(u32, String, PermissionChoice)>,
}

impl PermissionDialog {
    pub fn number_for(&self, choice: PermissionChoice) -> Option<u32> {
        self.options.iter().find(|o| o.2 == choice).map(|o| o.0)
    }
}

fn classify(label: &str) -> PermissionChoice {
    let l = label.trim();
    let lower = l.to_lowercase();
    if l == "Yes" {
        PermissionChoice::Allow
    } else if lower.starts_with("yes,")
        && !lower.contains("auto mode")
        && (lower.contains("always allow")
            || lower.contains("don't ask again")
            || lower.contains("allow all"))
    {
        PermissionChoice::AllowAlways
    } else if l == "No" || lower.starts_with("no,") || lower.starts_with("no ") {
        PermissionChoice::Deny
    } else {
        PermissionChoice::Other
    }
}

/// A tool permission dialog: a `Do you want …?` question followed by a numbered menu with a plain
/// `Yes` and a `No` option.
pub fn permission(lines: &[String]) -> Option<PermissionDialog> {
    let q = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with("Do you want"))?;
    let opts: Vec<MenuOption> = menu(&lines[q..])
        .into_iter()
        .map(|mut o| {
            o.row += q;
            o
        })
        .collect();
    if opts.is_empty() {
        return None;
    }
    let options: Vec<_> = opts
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let label = full_label(lines, o, opts.get(i + 1).map(|n| n.row));
            let c = classify(&label);
            (o.number, label, c)
        })
        .collect();
    let has = |c| options.iter().any(|o| o.2 == c);
    (has(PermissionChoice::Allow) && has(PermissionChoice::Deny))
        .then_some(PermissionDialog { options })
}

/// The AskUserQuestion UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskScreen {
    Question(AskQuestion),
    /// The final `Review your answers` tab; `submit` is the number of `Submit answers`.
    Review {
        submit: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskQuestion {
    /// Question text as shown.
    pub title: String,
    /// More than one question: a tab row with `←` / `→` and a Submit tab.
    pub tabbed: bool,
    /// Answer options, without the trailing `Type something` / `Chat about this` rows.
    pub options: Vec<MenuOption>,
    /// Number of the `Type something` row.
    pub other: Option<u32>,
    pub multi: bool,
    /// Screen row of the unnumbered `Submit` row of a multi-select question.
    pub submit_row: Option<usize>,
    /// Row index (0-based in `lines`) the `❯` cursor is on, if any.
    pub cursor_row: Option<usize>,
    /// Typing a free-text answer (`ctrl+g to edit in Vim` hint shown).
    pub editing: bool,
}

fn is_ask_footer(l: &str) -> bool {
    let t = l.trim();
    t.starts_with("Enter to select") && t.contains("Esc to cancel")
}

pub fn ask(lines: &[String]) -> Option<AskScreen> {
    if let Some(r) = lines
        .iter()
        .rposition(|l| l.trim() == "Review your answers")
    {
        let opts = menu(&lines[r..]);
        let submit = opts
            .iter()
            .find(|o| o.label.starts_with("Submit answers"))?
            .number;
        return Some(AskScreen::Review { submit });
    }
    let footer = lines.iter().rposition(|l| is_ask_footer(l))?;
    let editing = lines[footer].contains("edit in Vim");
    // The header row: `←  ☐ A  ☒ B  ✔ Submit  →` or ` ☐ A`.
    let header = lines[..footer]
        .iter()
        .rposition(|l| l.contains('☐') || l.contains('☒') || l.contains("✔ Submit"))?;
    let tabbed = lines[header].trim_start().starts_with('←');
    let title_row = (header + 1..footer).find(|&i| !lines[i].trim().is_empty())?;
    let title = lines[title_row].trim().to_string();
    let all: Vec<MenuOption> = menu(&lines[title_row..footer])
        .into_iter()
        .map(|mut o| {
            o.row += title_row;
            o
        })
        .collect();
    if all.is_empty() {
        return None;
    }
    let other = all
        .iter()
        .find(|o| o.label.starts_with("Type something"))
        .map(|o| o.number);
    let options: Vec<MenuOption> = all
        .iter()
        .filter(|o| {
            !o.label.starts_with("Type something") && !o.label.starts_with("Chat about this")
        })
        .cloned()
        .collect();
    let multi = options.iter().any(|o| o.checked.is_some());
    let submit_row = (title_row..footer).find(|&i| {
        let t = lines[i].trim_start();
        t == "Submit" || t == "❯ Submit"
    });
    let cursor_row = (title_row..footer).find(|&i| lines[i].trim_start().starts_with('❯'));
    // While typing, the `Type something.` row shows the typed text instead.
    let other = other.or_else(|| {
        editing
            .then(|| all.iter().find(|o| o.cursor).map(|o| o.number))
            .flatten()
    });
    Some(AskScreen::Question(AskQuestion {
        title,
        tabbed,
        options,
        other,
        multi,
        submit_row,
        cursor_row,
        editing,
    }))
}

/// The prompt input box: the text after `❯` between the two horizontal rules at the bottom.
/// `Some("")` when empty, `None` when no input box is visible (a dialog has the screen).
pub fn input_box(lines: &[String]) -> Option<String> {
    let rules: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with('─') && l.chars().all(|c| c == '─'))
        .map(|(i, _)| i)
        .collect();
    let (&bottom, rest) = rules.split_last()?;
    let &top = rest.last()?;
    if bottom <= top + 1 {
        return None;
    }
    let first = lines[top + 1].strip_prefix('❯')?;
    let mut s = first.trim().to_string();
    for l in &lines[top + 2..bottom] {
        s.push('\n');
        s.push_str(l.trim());
    }
    Some(s.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str) -> Vec<String> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/screens")
            .join(name);
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn permission_dialog_options_by_meaning() {
        let d = permission(&screen("permission_bash.txt")).unwrap();
        assert_eq!(d.number_for(PermissionChoice::Allow), Some(1));
        assert_eq!(d.number_for(PermissionChoice::AllowAlways), Some(2));
        assert_eq!(d.number_for(PermissionChoice::Deny), Some(4));
        assert_eq!(
            d.options[2].2,
            PermissionChoice::Other,
            "switch to auto mode is not allow_always"
        );
        assert!(
            d.options[1].1.ends_with("from this project"),
            "{}",
            d.options[1].1
        );
    }

    #[test]
    fn edit_style_permission_labels() {
        let lines: Vec<String> = [
            " Do you want to make this edit to a.txt?",
            " ❯ 1. Yes",
            "   2. Yes, allow all edits during this session (shift+tab)",
            "   3. No",
            "",
            " Esc to cancel · Tab to amend",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let d = permission(&lines).unwrap();
        assert_eq!(d.number_for(PermissionChoice::AllowAlways), Some(2));
        assert_eq!(d.number_for(PermissionChoice::Deny), Some(3));
    }

    #[test]
    fn non_dialog_screens_are_not_permission() {
        assert!(permission(&screen("ask_single.txt")).is_none());
        assert!(permission(&screen("idle_prompt.txt")).is_none());
    }

    #[test]
    fn ask_tabbed_single_choice() {
        let Some(AskScreen::Question(q)) = ask(&screen("ask_multi_q1.txt")) else {
            panic!()
        };
        assert_eq!(q.title, "Indent style?");
        assert!(q.tabbed && !q.multi && !q.editing);
        let labels: Vec<_> = q
            .options
            .iter()
            .map(|o| (o.number, o.label.as_str()))
            .collect();
        assert_eq!(labels, vec![(1, "Tabs"), (2, "Spaces")]);
        assert_eq!(q.other, Some(3));
    }

    #[test]
    fn ask_multi_select_state() {
        let Some(AskScreen::Question(q)) = ask(&screen("ask_multi_q2.txt")) else {
            panic!()
        };
        assert_eq!(q.title, "Which linters?");
        assert!(q.multi);
        assert_eq!(q.options.len(), 3);
        assert!(q.options.iter().all(|o| o.checked == Some(false)));
        assert_eq!(q.other, Some(4));
        assert!(q.submit_row.is_some());
        let Some(AskScreen::Question(q)) = ask(&screen("ask_multi_q2_checked.txt")) else {
            panic!()
        };
        let checked: Vec<_> = q
            .options
            .iter()
            .filter(|o| o.checked == Some(true))
            .map(|o| o.label.as_str())
            .collect();
        assert_eq!(checked, vec!["eslint", "biome"]);
    }

    #[test]
    fn ask_review_and_single() {
        assert_eq!(
            ask(&screen("ask_review.txt")),
            Some(AskScreen::Review { submit: 1 })
        );
        let Some(AskScreen::Question(q)) = ask(&screen("ask_single.txt")) else {
            panic!()
        };
        assert!(!q.tabbed);
        assert_eq!(q.title, "Favorite color?");
        let Some(AskScreen::Question(q)) = ask(&screen("ask_single_other.txt")) else {
            panic!()
        };
        assert!(q.editing);
        assert_eq!(q.other, Some(3));
    }

    #[test]
    fn input_box_contents() {
        assert_eq!(input_box(&screen("idle_prompt.txt")).as_deref(), Some(""));
        assert_eq!(
            input_box(&screen("idle_prompt_restored.txt")).as_deref(),
            Some("Reply with exactly the word PONG and nothing else.")
        );
        assert_eq!(input_box(&screen("ask_single.txt")), None);
    }
}
