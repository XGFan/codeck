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
    // Claude Code writes `don’t` with a typographic apostrophe.
    let lower = l.to_lowercase().replace('’', "'");
    // `Yes, run it`: the Workflow dialog's plain yes.
    if l == "Yes" || l == "Yes, run it" {
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

/// The question line of a permission dialog and the row its menu starts on. A tool dialog asks
/// `Do you want …?` right above the menu (blank lines aside); the Workflow dialog asks `Run a
/// dynamic workflow?` above the script's description, which may hold numbered phases itself, so
/// its menu is the last numbered block starting at 1.
fn permission_question(lines: &[String]) -> Option<(usize, usize)> {
    let q = lines.iter().rposition(|l| {
        let t = l.trim_start();
        t.starts_with("Do you want") || t.trim_end() == "Run a dynamic workflow?"
    })?;
    if lines[q].trim_start().starts_with("Do you want") {
        let mut i = q + 1;
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }
        return Some((q, i));
    }
    Some((q, last_menu_start(lines, q)?))
}

/// A strictly sequential numbered menu starting on row `i`: option 1 first, each next option on
/// the following row or after continuation rows indented deeper than the number, exactly one
/// `❯`, and no further numbered row below it. `(number, full label)`; anything else is refused.
pub fn strict_menu(lines: &[String], mut i: usize) -> Option<Vec<(u32, String)>> {
    let mut opts: Vec<(u32, String, bool, usize)> = Vec::new(); // number, label, cursor, indent
    while i < lines.len() {
        let line = &lines[i];
        if line.trim().is_empty() {
            break;
        }
        match parse_option(line) {
            Some((n, cursor, label)) if n as usize == opts.len() + 1 => {
                let indent = number_column(line);
                opts.push((n, label, cursor, indent));
            }
            Some(_) => return None,
            None => {
                let (_, _, _, indent) = opts.last()?;
                let lead = line.len() - line.trim_start().len();
                if lead <= *indent {
                    return None;
                }
                let last = opts.last_mut()?;
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        }
        i += 1;
    }
    if opts.len() < 2 || opts.iter().filter(|o| o.2).count() != 1 {
        return None;
    }
    // Below the menu only the footer may follow; another numbered row means we misread.
    if lines[i..].iter().any(|l| parse_option(l).is_some()) {
        return None;
    }
    Some(opts.into_iter().map(|(n, l, _, _)| (n, l)).collect())
}

/// Row of the last option numbered 1 after row `from` (the menu of a dialog whose body may
/// hold numbered lines itself).
pub fn last_menu_start(lines: &[String], from: usize) -> Option<usize> {
    (from + 1..lines.len())
        .rev()
        .find(|&i| parse_option(&lines[i]).is_some_and(|(n, _, _)| n == 1))
}

/// A tool permission dialog (see [`permission_question`]): a [`strict_menu`] with `No…` last and
/// a plain `Yes` present. Anything else is refused.
pub fn permission(lines: &[String]) -> Option<PermissionDialog> {
    let (_, i) = permission_question(lines)?;
    let options: Vec<_> = strict_menu(lines, i)?
        .into_iter()
        .map(|(n, l)| {
            let c = classify(&l);
            (n, l, c)
        })
        .collect();
    let has = |c| options.iter().any(|o| o.2 == c);
    let deny_last = options
        .last()
        .is_some_and(|o| o.2 == PermissionChoice::Deny);
    (has(PermissionChoice::Allow) && deny_last).then_some(PermissionDialog { options })
}

/// Byte column where an option's number starts (after any `❯` marker).
fn number_column(line: &str) -> usize {
    let t = line.trim_start();
    let lead = line.len() - t.len();
    match t.strip_prefix('❯') {
        Some(rest) => lead + '❯'.len_utf8() + (rest.len() - rest.trim_start().len()),
        None => lead,
    }
}

/// The permission dialog's body (from the rule above it to the menu) with all whitespace removed,
/// for matching the requested tool's command or file against what is shown.
pub fn permission_body(lines: &[String]) -> Option<String> {
    let (q, menu) = permission_question(lines)?;
    let top = lines[..q]
        .iter()
        .rposition(|l| l.starts_with('─') && l.chars().all(|c| c == '─'))?;
    Some(
        lines[top + 1..menu]
            .iter()
            .flat_map(|l| l.chars())
            // `│` frames a multi-line command; it is not part of it.
            .filter(|c| !c.is_whitespace() && *c != '│')
            .collect(),
    )
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
    // A long question wraps: the title is every line up to the first blank one.
    let title_end = (title_row..footer)
        .find(|&i| lines[i].trim().is_empty())
        .unwrap_or(footer);
    let title = collapse_ws(&lines[title_row..title_end].join(" "));
    let all: Vec<MenuOption> = menu(&lines[title_end..footer])
        .into_iter()
        .map(|mut o| {
            o.row += title_end;
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

/// Whitespace runs folded to one space, trimmed (screen text wraps and pads).
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
    fn multi_line_command_and_typographic_apostrophe() {
        let lines = screen("permission_multiline_bash.txt");
        let d = permission(&lines).unwrap();
        assert_eq!(d.number_for(PermissionChoice::AllowAlways), Some(2));
        assert_eq!(d.number_for(PermissionChoice::Deny), Some(4));
        let command = "python3 -I -c \"\nfrom PIL import Image\nim = Image.open('/private/var";
        let needle: String = command.chars().filter(|c| !c.is_whitespace()).take(40).collect();
        assert!(permission_body(&lines).unwrap().contains(&needle));
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn permission_mode_from_the_footer() {
        use crate::rpc::PermissionMode;
        let base = |footer: &str| lines(&["❯ hello", "─────", "❯", "─────", footer]);
        assert_eq!(permission_mode(&base("  ⏸ manual mode on · ? for shortcuts")), Some(PermissionMode::Manual));
        assert_eq!(permission_mode(&base("  ⏵⏵ accept edits on (shift+tab to cycle)")), Some(PermissionMode::AcceptEdits));
        assert_eq!(permission_mode(&base("  ⏸ plan mode on (shift+tab to cycle)")), Some(PermissionMode::Plan));
        assert_eq!(permission_mode(&base("  ⏵⏵ auto mode on (shift+tab to cycle)")), Some(PermissionMode::Auto));
        assert_eq!(permission_mode(&base("  ? for shortcuts")), None);
    }

    #[test]
    fn tasks_panel_list_and_details() {
        let list = lines(&[
            "──────",
            "  Background",
            "  2 active shells",
            "",
            "  ❯ ⏺ sleep 25   running",
            "    ⏺ npm run dev   running",
            "",
            "  ↑/↓ to select · Enter to view · x to stop · Esc to close",
        ]);
        assert_eq!(
            tasks_panel(&list),
            Some(TasksPanel::List(vec![
                ("⏺ sleep 25   running".into(), true),
                ("⏺ npm run dev   running".into(), false),
            ]))
        );
        let details = lines(&[
            "   Shell details",
            "   Status:   running",
            "   Command:  sleep 25",
            "   ← to go back · Esc/Enter/Space to close · x to stop",
        ]);
        assert!(matches!(tasks_panel(&details), Some(TasksPanel::Details(t)) if t.contains("sleep 25")));
        // Nothing left to stop: still the panel (to be closed).
        let done = lines(&[
            "   Background",
            "     Completed (1)",
            "   ❯ ✔ Echo test command   done · deepseek-flash",
            "   ↑/↓ to select · Enter to view · Esc to close",
        ]);
        assert!(matches!(tasks_panel(&done), Some(TasksPanel::List(rows)) if rows.len() == 2));
    }

    #[test]
    fn workflow_dialog_menu_after_numbered_phases() {
        let lines = screen("permission_workflow.txt");
        let d = permission(&lines).unwrap();
        assert_eq!(d.number_for(PermissionChoice::Allow), Some(1));
        assert_eq!(d.number_for(PermissionChoice::Deny), Some(3));
        assert_eq!(d.number_for(PermissionChoice::AllowAlways), None);
        assert!(permission_body(&lines).unwrap().contains("echo"));
    }

    #[test]
    fn permission_menu_must_be_strict() {
        let base = screen("permission_bash.txt");
        // a stray numbered row after the menu (e.g. tool text) makes it ambiguous
        let mut extra = base.clone();
        extra.push("  5. Yes".into());
        assert!(permission(&extra).is_none());
        // a gap in numbering
        let gap: Vec<String> = base
            .iter()
            .map(|l| l.replace("3. Yes, and switch", "7. Yes, and switch"))
            .collect();
        assert!(permission(&gap).is_none());
        // no cursor
        let nocur: Vec<String> = base
            .iter()
            .map(|l| l.replace("❯ 1. Yes", "  1. Yes"))
            .collect();
        assert!(permission(&nocur).is_none());
        // body for matching
        let body = permission_body(&base).unwrap();
        assert!(body.contains("touchcreated_by_probe.txt"), "{body}");
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

/// The permission mode shown in the footer below the input box (`⏸ manual mode on`, `⏵⏵ accept
/// edits on`, `⏸ plan mode on`, `⏵⏵ auto mode on`).
pub fn permission_mode(lines: &[String]) -> Option<crate::rpc::PermissionMode> {
    use crate::rpc::PermissionMode::*;
    let rule = lines
        .iter()
        .rposition(|l| l.starts_with('─') && l.chars().all(|c| c == '─'))?;
    lines[rule + 1..].iter().find_map(|l| {
        let l = l.to_lowercase();
        if l.contains("manual mode on") {
            Some(Manual)
        } else if l.contains("accept edits on") {
            Some(AcceptEdits)
        } else if l.contains("plan mode on") {
            Some(Plan)
        } else if l.contains("auto mode on") {
            Some(Auto)
        } else if l.contains("bypass permissions on") {
            Some(Bypass)
        } else {
            None
        }
    })
}

/// The background tasks panel (`/tasks`): a list with a `❯` cursor, or one task's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TasksPanel {
    /// Rows `(text, selected)` from top to bottom.
    List(Vec<(String, bool)>),
    /// The text of a task's details view.
    Details(String),
}

/// Recognizes the panel by its hint line (`… x to stop …`, or `… Esc to close` once nothing is
/// left to stop): `↑/↓ to select` marks the list.
pub fn tasks_panel(lines: &[String]) -> Option<TasksPanel> {
    let hint = lines
        .iter()
        .rposition(|l| l.contains("x to stop") || l.contains("Esc to close") || l.contains("Esc/Enter/Space to close"))?;
    if lines[hint].contains("to select") {
        let title = lines[..hint]
            .iter()
            .rposition(|l| l.trim() == "Background")?;
        let rows = lines[title + 1..hint]
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .filter(|l| l.split_whitespace().nth(1).is_none_or(|w| w != "active"))
            .map(|l| match l.strip_prefix('❯') {
                Some(rest) => (rest.trim().to_string(), true),
                None => (l.to_string(), false),
            })
            .collect();
        return Some(TasksPanel::List(rows));
    }
    let title = lines[..hint]
        .iter()
        .rposition(|l| l.trim().ends_with("details"))?;
    Some(TasksPanel::Details(lines[title..hint].join("\n")))
}

/// A panel row's text without its leading status glyphs (`⏺`, `◯`, `✔`, …), whitespace collapsed.
fn row_text(row: &str) -> String {
    collapse_ws(row.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.'))
}

/// Row of the panel list to stop: the one running row that starts with `needle` (a shell's
/// command, an agent's description). `Err` with what is wrong: no such row, more than one, or an
/// ended row that starts the same (the target may have ended and another one be running).
pub fn task_row(rows: &[(String, bool)], needle: &str) -> Result<usize, &'static str> {
    let needle = collapse_ws(needle);
    if needle.is_empty() {
        return Err("没有可比对的命令或描述");
    }
    let fits = |t: &str| row_text(t).starts_with(&needle);
    let running = |t: &str| t.split_whitespace().any(|w| w == "running");
    let hits: Vec<usize> = (0..rows.len())
        .filter(|&i| fits(&rows[i].0) && running(&rows[i].0))
        .collect();
    if rows.iter().any(|(t, _)| fits(t) && !running(t)) {
        return Err("面板上有同样的已结束任务，分不清是哪一个");
    }
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => Err("面板上没有对得上的运行中任务"),
        _ => Err("面板上有多个对得上的运行中任务"),
    }
}

#[cfg(test)]
mod task_row_tests {
    use super::*;

    fn rows(v: &[&str]) -> Vec<(String, bool)> {
        v.iter().enumerate().map(|(i, s)| (s.to_string(), i == 0)).collect()
    }

    #[test]
    fn exactly_one_running_row() {
        let r = rows(&["Shells (2)", "⏺ npm run dev   running", "⏺ sleep 300   running", "Local agents (1)", "◯ wait for deploy   running · m"]);
        assert_eq!(task_row(&r, "sleep 300"), Ok(2));
        assert_eq!(task_row(&r, "wait for deploy"), Ok(4));
        // Only a whole leading match: `dev` is not `npm run dev`.
        assert!(task_row(&r, "dev").is_err());
        assert!(task_row(&r, "sleep 9").is_err());
        let two = rows(&["⏺ npm run dev   running", "⏺ npm run dev   running"]);
        assert!(task_row(&two, "npm run dev").is_err());
        // The same command already ended: the target may be that one.
        let ended = rows(&["⏺ sleep 300   running", "Completed (1)", "✔ sleep 300   done"]);
        assert!(task_row(&ended, "sleep 300").is_err());
    }
}
