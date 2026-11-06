//! Screen recognizers for the interactive UIs ccrpc answers (layouts of Claude Code 2.1.316 / 2.1.317, see
//! docs/probes.md). Each one returns `None` when the screen does not match what it knows; callers
//! then report `unrecognized` instead of pressing keys.

use crate::layout::{Meaning, Select};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDialog {
    /// `(number, full label, meaning)`; in a cursor menu the number is the position.
    pub options: Vec<(u32, String, Meaning)>,
    pub select: Select,
    /// The option `❯` is on (read for a cursor menu, answered by moving it).
    pub cursor: Option<u32>,
    /// The tool that draws the dialog itself (see `Layout::tool`).
    pub tool: Option<String>,
}

impl PermissionDialog {
    pub fn number_for(&self, choice: Meaning) -> Option<u32> {
        self.options.iter().find(|o| o.2 == choice).map(|o| o.0)
    }
}

fn classify(label: &str) -> Meaning {
    let l = label.trim();
    // Claude Code writes `don’t` with a typographic apostrophe.
    let lower = l.to_lowercase().replace('’', "'");
    // `Yes, run it`: the Workflow dialog's plain yes.
    if l == "Yes" || l == "Yes, run it" {
        Meaning::Allow
    } else if lower.starts_with("yes,")
        && !lower.contains("auto mode")
        && (lower.contains("always allow")
            || lower.contains("don't ask again")
            || lower.contains("allow all"))
    {
        Meaning::AllowAlways
    } else if l == "No" || lower.starts_with("no,") || lower.starts_with("no ") {
        Meaning::Deny
    } else {
        Meaning::Other
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
pub fn strict_menu(lines: &[String], i: usize) -> Option<Vec<(u32, String)>> {
    Some(
        strict_menu_rows(lines, i)?
            .into_iter()
            .map(|(n, l, _)| (n, l))
            .collect(),
    )
}

/// [`strict_menu`] with whether the `❯` cursor is on each option.
pub fn strict_menu_rows(lines: &[String], mut i: usize) -> Option<Vec<(u32, String, bool)>> {
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
    Some(opts.into_iter().map(|(n, l, c, _)| (n, l, c)).collect())
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
    let deny_last = options.last().is_some_and(|o| o.2 == Meaning::Deny);
    (has(Meaning::Allow) && deny_last).then_some(PermissionDialog {
        options,
        select: Select::Number,
        cursor: None,
        tool: None,
    })
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

/// What a row of a `╭…╯` box holds, without its frame.
fn box_text(l: &str) -> &str {
    l.trim()
        .trim_start_matches('│')
        .trim_end_matches('│')
        .trim()
}

/// The feedback draft card's key row (the draft goes out only by one of these keys).
const KEYS: &str = "1 to review · 2 to send · 0 to dismiss";

/// The card above the input box once the model drafted product feedback (`SendFeedback`, 2.1.318).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackCard {
    /// The title as its row shows it (cut, ending `…`, on a narrow screen).
    pub title: String,
    /// The row of the card's bottom frame.
    pub bottom: usize,
}

/// The feedback draft card: a box drawn from column 0 starting `✻ Product feedback drafted: <title>`,
/// its last row the keys `1 to review · 2 to send · 0 to dismiss` (then `· +1 more queued` while
/// more drafts wait) right above its bottom frame, and
/// below it only blank rows or the effort mark until the input box's top rule or the end of the
/// screen (on a short screen the card pushes the input box off it). The same text quoted in the
/// conversation is indented and has the conversation below it: not the card.
pub fn feedback_draft(lines: &[String]) -> Option<FeedbackCard> {
    let keys = lines.iter().rposition(|l| {
        let t = box_text(l);
        t == KEYS
            || t.strip_prefix(KEYS)
                .is_some_and(|rest| rest.starts_with(" · "))
    })?;
    let bottom = keys + 1;
    if !lines.get(bottom)?.starts_with('╰') {
        return None;
    }
    let below_ok = lines[bottom + 1..]
        .iter()
        .find(|l| !l.trim().is_empty() && effort(std::slice::from_ref(*l)).is_none())
        .is_none_or(|l| is_input_rule(l));
    if !below_ok {
        return None;
    }
    let top = lines[..keys].iter().rposition(|l| l.starts_with('╭'))?;
    let title = lines[top + 1..keys].iter().find_map(|l| {
        box_text(l)
            .strip_prefix("✻ Product feedback drafted:")
            .map(|t| t.trim().to_string())
    })?;
    Some(FeedbackCard { title, bottom })
}

/// The input box below row `below` is empty and has the cursor: one row, nothing typed after
/// `❯` (spaces or a line break typed there would make a key text), the cursor right after it.
/// `None` when no input box shows below that row.
pub fn input_empty_below(s: &crate::screen::Screen, below: usize) -> Option<bool> {
    if agents_page(&s.lines) || (s.moved && !mode_line_under_box(&s.lines)) {
        return None;
    }
    let rules: Vec<usize> = (below + 1..s.lines.len())
        .filter(|&i| is_input_rule(&s.lines[i]))
        .collect();
    let (&top, &bottom) = (rules.first()?, rules.get(1)?);
    if bottom == top + 1 {
        return None;
    }
    let rows = if s.undimmed.len() == s.lines.len() {
        &s.undimmed
    } else {
        &s.lines
    };
    let empty =
        bottom == top + 2 && rows[top + 1].trim_end() == "❯" && s.cursor == ((top + 1) as u16, 2);
    Some(empty)
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
    // 2.1.318 draws the question in a `│ ` quote bar.
    let title = collapse_ws(
        &lines[title_row..title_end]
            .iter()
            .map(|l| l.trim_start().strip_prefix('│').unwrap_or(l))
            .collect::<Vec<_>>()
            .join(" "),
    );
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

/// The same text once wrapping is undone: a line break in CJK text adds no space, in other text it
/// replaces one, so every whitespace is dropped before comparing.
pub fn same_text(a: &str, b: &str) -> bool {
    a.chars()
        .filter(|c| !c.is_whitespace())
        .eq(b.chars().filter(|c| !c.is_whitespace()))
}

/// Whitespace runs folded to one space, trimmed (screen text wraps and pads).
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A rule of the input box: all `─`, or with a title near its right end (`──── probe-a ─`: the
/// top rule of a Session named with `/rename`).
pub fn is_input_rule(l: &str) -> bool {
    let l = l.trim_end();
    if !l.starts_with('─') || !l.ends_with('─') {
        return false;
    }
    let head = l.trim_end_matches('─');
    if head.is_empty() {
        return true;
    }
    let tail = l[head.len()..].chars().count();
    let body = head.trim_start_matches('─');
    let lead = head[..head.len() - body.len()].chars().count();
    let Some(title) = body.strip_prefix(' ').and_then(|t| t.strip_suffix(' ')) else {
        return false;
    };
    // The title sits at the right end (a dialog's `──── Title ────` frame is not an input box rule).
    !title.trim().is_empty() && !title.contains('─') && tail <= 3 && lead >= 4
}

/// The agents page (`←` on an empty input box, 2.1.318, `tests/fixtures/screens/agents_list.txt`):
/// its box at the bottom looks like the input box but starts a new background Session. Known by
/// its box's placeholder, the key hints under it (they change with the selection and once
/// something is typed), the note at its top, or its section headings at the left edge (`Working`,
/// `Working 2` when folded; the conversation's own lines are indented or start with a mark).
pub fn agents_page(lines: &[String]) -> bool {
    const HINTS: [&str; 4] = [
        "ctrl+x to delete",
        "ctrl+x to stop",
        "ctrl+enter to start and open",
        "space to reply",
    ];
    const HEADINGS: [&str; 3] = ["Needs input", "Working", "Completed"];
    let heading = |l: &str| {
        HEADINGS.iter().any(|h| {
            l.strip_prefix(h).is_some_and(|rest| {
                rest.is_empty()
                    || rest
                        .strip_prefix(' ')
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
        })
    };
    if let Some(bottom) = lines.iter().rposition(|l| is_input_rule(l))
        && lines[bottom + 1..]
            .iter()
            .any(|l| HINTS.iter().any(|h| l.contains(h)))
    {
        return true;
    }
    lines.iter().any(|l| {
        l.trim_start() == "❯ describe a task for a new session"
            || l.starts_with("Your conversation moved to the background")
            || heading(l.trim_end())
    })
}

/// The line under the input box is the mode line (`⏸ manual mode on · ← for agents`): the box is
/// the conversation's, not the agents page's.
pub fn mode_line_under_box(lines: &[String]) -> bool {
    const MARKS: [&str; 6] = [
        "manual mode on",
        "accept edits on",
        "plan mode on",
        "auto mode on",
        "bypass permissions on",
        "for agents",
    ];
    let Some(bottom) = lines.iter().rposition(|l| is_input_rule(l)) else {
        return false;
    };
    lines.get(bottom + 1).is_some_and(|l| {
        let l = l.to_lowercase();
        MARKS.iter().any(|m| l.contains(m))
    })
}

/// The prompt input box: the text after `❯` between the two horizontal rules at the bottom; in
/// shell mode (`!` instead of `❯`) the text starts with `!` (`"!"` when empty). `Some("")` when
/// empty, `None` when no input box is visible (a dialog has the screen, or the agents page).
pub fn input_box(lines: &[String]) -> Option<String> {
    let rules: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_input_rule(l))
        .map(|(i, _)| i)
        .collect();
    let (&bottom, rest) = rules.split_last()?;
    let &top = rest.last()?;
    if bottom <= top + 1 || agents_page(lines) {
        return None;
    }
    // `❯`, or `!` in shell mode (its text then starts with `!`).
    let row = &lines[top + 1];
    let (shell, first) = match row.strip_prefix('❯') {
        Some(f) => (false, f),
        None => (true, row.strip_prefix('!')?),
    };
    let mut s = first.trim().to_string();
    for l in &lines[top + 2..bottom] {
        s.push('\n');
        s.push_str(l.trim());
    }
    let s = s.trim_end().to_string();
    Some(if shell { format!("!{s}") } else { s })
}

/// The input box is in shell mode (`!` instead of `❯`): its command, else `None` (no box, or a
/// normal one, even one holding text that starts with `!`).
pub fn shell_input(s: &crate::screen::Screen) -> Option<String> {
    // The first column as drawn (a dim `❯` is still the normal box).
    let lines = &s.lines;
    let rules: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_input_rule(l))
        .map(|(i, _)| i)
        .collect();
    let (&bottom, rest) = rules.split_last()?;
    let &top = rest.last()?;
    if bottom <= top + 1 || lines[top + 1].starts_with('❯') {
        return None;
    }
    input_text(s)?.strip_prefix('!').map(str::to_string)
}

/// [`input_box`] without what Claude Code draws dim in it: an empty box shows a placeholder such
/// as `Try "edit <filepath> to..."` that ctrl+u cannot clear. The box is found on the full text.
pub fn input_text(s: &crate::screen::Screen) -> Option<String> {
    // Moved to the background, the Pane may show the agents page with hints not seen yet: only
    // a box with the mode line under it is the conversation's.
    if s.moved && !mode_line_under_box(&s.lines) {
        return None;
    }
    let full = input_box(&s.lines)?;
    if s.undimmed.len() != s.lines.len() {
        return Some(full);
    }
    Some(input_box(&s.undimmed).unwrap_or(full))
}

/// The folder trust dialog shown before a Session starts in a folder not trusted yet: an
/// unnumbered menu (`❯` marks the selected row) confirmed with Enter. `Some(true)` when the cursor
/// is on "Yes, I trust this folder".
pub fn trust_folder(lines: &[String]) -> Option<bool> {
    let text = collapse_ws(&lines.join(" "));
    if !text.contains("Is this a project you created or one you trust?")
        || !text.contains("Enter to confirm")
    {
        return None;
    }
    let row = |label: &str| {
        lines.iter().find_map(|l| {
            let t = l.trim();
            let (sel, rest) = match t.strip_prefix('❯') {
                Some(r) => (true, r.trim_start()),
                None => (false, t),
            };
            (rest == label).then_some(sel)
        })
    };
    let (yes, no) = (row(TRUST_YES)?, row(TRUST_NO)?);
    (yes != no).then_some(yes)
}

pub const TRUST_YES: &str = "Yes, I trust this folder";
pub const TRUST_NO: &str = "No, exit";
pub const TRUST_TITLE: &str = "Is this a project you created or one you trust?";

/// The option of [`exit_with_tasks`] that stops the tasks and exits.
pub const EXIT_STOP_TASKS: &str = "Exit and stop tasks";

/// What `/exit` asks while background tasks run (2.1.318, `tests/fixtures/screens/exit_with_tasks.txt`):
/// `Background work is running` above a [`strict_menu`] holding `Exit and stop tasks`, then
/// `Enter to confirm · Esc to cancel`. `Some(true)` when the cursor is on `Exit and stop tasks`;
/// anything else is refused.
pub fn exit_with_tasks(lines: &[String]) -> Option<bool> {
    let q = lines
        .iter()
        .rposition(|l| l.trim() == "Background work is running")?;
    let start = last_menu_start(lines, q)?;
    let (_, _, cursor) = strict_menu_rows(lines, start)?
        .into_iter()
        .find(|(_, label, _)| label == EXIT_STOP_TASKS)?;
    let footer = lines[start..].iter().any(|l| {
        let t = l.trim();
        t.starts_with("Enter to confirm") && t.contains("Esc to cancel")
    });
    footer.then_some(cursor)
}

/// How many pasted images the input box text shows (`[Image #n]` placeholders).
pub fn image_placeholders(input: &str) -> usize {
    input.matches("[Image #").count()
}

/// The `❯` cursor is on the unnumbered `Submit` row of a multi-select question.
pub fn on_submit_row(line: &str) -> bool {
    line.trim_start().starts_with("❯ Submit")
}

/// The spinner line above the input box while Claude Code works, such as
/// `✻ Deliberating… (7s · ↓ 10 tokens)` or `✶ Brewing… (running PreToolUse hook · 2s · ↓ 46 tokens)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spinner {
    /// The line without its glyph.
    pub text: String,
    /// The word before `…` (`Deliberating`).
    pub verb: String,
    pub elapsed_s: Option<u64>,
    pub tokens: Option<u64>,
}

pub fn spinner(lines: &[String]) -> Option<Spinner> {
    const GLYPHS: &[char] = &['✻', '✶', '✳', '✢', '·', '✽', '*'];
    lines.iter().rev().find_map(|l| {
        let t = l.trim_start();
        let mut cs = t.chars();
        let g = cs.next()?;
        if !GLYPHS.contains(&g) || cs.next() != Some(' ') {
            return None;
        }
        let text = t[g.len_utf8() + 1..].trim().to_string();
        let (verb, rest) = text.split_once('…')?;
        if verb.is_empty() || verb.contains(' ') {
            return None;
        }
        let inner = rest
            .trim()
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or("");
        let mut elapsed_s = None;
        let mut tokens = None;
        for part in inner.split('·').map(str::trim) {
            if let Some(n) = part.strip_suffix(" tokens") {
                tokens = n
                    .trim_start_matches(['↓', '↑'])
                    .trim()
                    .replace(',', "")
                    .parse()
                    .ok();
            } else if let Some(secs) = duration_secs(part) {
                elapsed_s = Some(secs);
            }
        }
        Some(Spinner {
            text: text.clone(),
            verb: verb.to_string(),
            elapsed_s,
            tokens,
        })
    })
}

/// `7s`, `1m 3s`, `2h 5m` as seconds.
fn duration_secs(s: &str) -> Option<u64> {
    let mut total = 0;
    let mut any = false;
    for p in s.split_whitespace() {
        let (n, unit) = p.split_at(p.find(|c: char| !c.is_ascii_digit())?);
        let n: u64 = n.parse().ok()?;
        total += n * match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            _ => return None,
        };
        any = true;
    }
    any.then_some(total)
}

/// The Rewind screens' title (2.1.318; `tests/fixtures/screens/rewind_*.txt`).
const REWIND_TITLE: &str = "Rewind";
const REWIND_PICKER_SUB: &str = "Restore the code and/or conversation to the point before…";
const REWIND_PICKER_FOOT: &str = "Enter to continue · Esc to cancel";
const REWIND_CONFIRM_SUB: &str =
    "Confirm you want to restore to the point before you sent this message:";
/// The picker's last row: the conversation as it is.
pub const REWIND_CURRENT: &str = "(current)";

/// The Rewind picker (`Esc Esc` on an empty input box): one row per prompt, oldest first, ending
/// with [`REWIND_CURRENT`]; each shows the prompt's first line (cut with `…`) above what its turn
/// changed. Only a window of rows is drawn: `above` / `below` count the rest (`↑ 8 more`).
#[derive(Debug, Clone, PartialEq)]
pub struct RewindPicker {
    /// Shown rows' text (without `❯`) and whether the cursor is on it.
    pub rows: Vec<(String, bool)>,
    pub above: usize,
    pub below: usize,
}

impl RewindPicker {
    /// The row under the cursor.
    pub fn cursor(&self) -> Option<&str> {
        self.rows.iter().find(|r| r.1).map(|r| r.0.as_str())
    }

    /// The cursor's row counted from the top of the whole list.
    pub fn position(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.1).map(|i| self.above + i)
    }
}

/// Row after the subtitle `sub` of a Rewind screen: the bottom-most `Rewind` title drawn right
/// under the dialog rule (`▔▔▔`), with no input box on the screen (a prompt or a reply quoting
/// these words is not the screen).
fn rewind_screen(lines: &[String], sub: &str) -> Option<usize> {
    if input_box(lines).is_some() {
        return None;
    }
    let t = (1..lines.len())
        .rev()
        .find(|&i| lines[i].trim() == REWIND_TITLE && lines[i - 1].trim_start().starts_with('▔'))?;
    let s = (t + 1..lines.len()).find(|&i| !lines[i].trim().is_empty())?;
    (lines[s].trim() == sub).then_some(s + 1)
}

/// `↑ 8 more` / `↓ 3 more`: (up, count).
fn more_marker(t: &str) -> Option<(bool, usize)> {
    let (up, rest) = match (t.strip_prefix("↑ "), t.strip_prefix("↓ ")) {
        (Some(r), _) => (true, r),
        (_, Some(r)) => (false, r),
        _ => return None,
    };
    Some((up, rest.strip_suffix(" more")?.trim().parse().ok()?))
}

pub fn rewind_picker(lines: &[String]) -> Option<RewindPicker> {
    let start = rewind_screen(lines, REWIND_PICKER_SUB)?;
    let foot = (start..lines.len()).find(|&i| lines[i].trim() == REWIND_PICKER_FOOT)?;
    let mut p = RewindPicker {
        rows: Vec::new(),
        above: 0,
        below: 0,
    };
    // Rows come in blank-separated groups: the prompt's line, then what its turn changed.
    let mut first_of_group = true;
    for line in &lines[start..foot] {
        let t = line.trim();
        if t.is_empty() {
            first_of_group = true;
            continue;
        }
        if let Some((up, n)) = more_marker(t) {
            if up {
                p.above = n
            } else {
                p.below = n
            }
            continue;
        }
        if first_of_group {
            let (cursor, text) = match t.strip_prefix('❯') {
                Some(r) => (true, r.trim_start()),
                None => (false, t),
            };
            p.rows.push((text.to_string(), cursor));
            first_of_group = false;
        }
    }
    (!p.rows.is_empty() && p.rows.iter().filter(|r| r.1).count() == 1).then_some(p)
}

/// The Rewind confirm page (Enter on a picker row): the prompt quoted (at most its first rows,
/// wrapped) and when it was sent, what happens to the code, the numbered options (a long menu
/// scrolls: a `↓ 5.` row), and a warning when code would be restored.
#[derive(Debug, Clone, PartialEq)]
pub struct RewindConfirm {
    /// Quoted rows without `│ `, leaving out the `(5s ago)` row.
    pub quote: Vec<String>,
    /// When the prompt was sent: `5s ago`.
    pub ago: Option<String>,
    /// `The code will be restored -1 in w.txt.` / `The code will be unchanged.`
    pub code: Option<String>,
    /// The `⚠` line, without the sign.
    pub warning: Option<String>,
    /// Shown options: number, label, cursor on it.
    pub options: Vec<(u32, String, bool)>,
}

impl RewindConfirm {
    /// The same page (wherever its cursor is, however long ago it says).
    pub fn same_page(&self, other: &RewindConfirm) -> bool {
        let labels = |c: &RewindConfirm| -> Vec<(u32, String)> {
            c.options.iter().map(|o| (o.0, o.1.clone())).collect()
        };
        // The age (`5s ago`) ticks on redraws: not part of the page.
        self.quote == other.quote
            && self.code == other.code
            && self.warning == other.warning
            && labels(self) == labels(other)
    }
}

pub fn rewind_confirm(lines: &[String]) -> Option<RewindConfirm> {
    let start = rewind_screen(lines, REWIND_CONFIRM_SUB)?;
    let mut c = RewindConfirm {
        quote: Vec::new(),
        ago: None,
        code: None,
        warning: None,
        options: Vec::new(),
    };
    let mut i = start;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    while i < lines.len() {
        let Some(q) = lines[i].trim_start().strip_prefix('│') else {
            break;
        };
        c.quote
            .push(q.strip_prefix(' ').unwrap_or(q).trim_end().to_string());
        i += 1;
    }
    if let Some(ago) = c.quote.last().and_then(|q| {
        let q = q.trim();
        q.strip_prefix('(')?
            .strip_suffix(')')
            .filter(|a| a.ends_with(" ago"))
    }) {
        c.ago = Some(ago.to_string());
        c.quote.pop();
    }
    if c.quote.is_empty() {
        return None;
    }
    // What follows: paragraphs (the code line, the warning) and the menu; any of them may be
    // wrapped onto more rows, which continue the paragraph or the option above.
    enum Para {
        None,
        Code,
        Warning,
        Option,
    }
    let mut para = Para::None;
    for line in &lines[i..] {
        let t = line.trim();
        if t.is_empty() {
            para = Para::None;
            continue;
        }
        let row = t.trim_start_matches(['❯', '↑', '↓']).trim_start();
        let cursor = t.starts_with('❯');
        if let Some((n, label)) = row
            .split_once(". ")
            .and_then(|(n, l)| Some((n.parse::<u32>().ok()?, l)))
        {
            c.options.push((n, label.trim().to_string(), cursor));
            para = Para::Option;
            continue;
        }
        if let Some(w) = t.strip_prefix('⚠') {
            c.warning = Some(w.trim().to_string());
            para = Para::Warning;
            continue;
        }
        if t.starts_with("The code will be") {
            c.code = Some(t.to_string());
            para = Para::Code;
            continue;
        }
        let more = match para {
            Para::None => continue,
            Para::Code => c.code.as_mut(),
            Para::Warning => c.warning.as_mut(),
            Para::Option => c.options.last_mut().map(|o| &mut o.1),
        };
        if let Some(p) = more {
            p.push(' ');
            p.push_str(t);
        }
    }
    let numbered = c
        .options
        .iter()
        .enumerate()
        .all(|(k, o)| k == 0 || o.0 == c.options[k - 1].0 + 1);
    (!c.options.is_empty() && numbered && c.options.iter().filter(|o| o.2).count() == 1)
        .then_some(c)
}

/// What the Rewind screens show of a prompt, for comparing: `[Image #N]` tags and all whitespace
/// left out (the picker trims, the confirm page wraps and breaks long words).
pub fn rewind_key(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("[Image #") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "[Image #".len()..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && tail[digits..].starts_with(']') {
            rest = &tail[digits + 1..];
        } else {
            out.push_str("[Image #");
            rest = tail;
        }
    }
    out.push_str(rest);
    out.retain(|c| !c.is_whitespace());
    out
}

/// The prompt's first line as the picker shows it (leading blank lines left out).
pub fn rewind_first_line(text: &str) -> &str {
    text.trim_start().lines().next().unwrap_or_default()
}

/// A picker row may show this prompt: its first line whole, or cut (a row ending in `…`: the
/// first of several lines, or cut at the screen's edge).
pub fn rewind_row_matches(row: &str, text: &str) -> bool {
    let shown = rewind_key(row.trim_end_matches('…'));
    let want = rewind_key(rewind_first_line(text));
    shown == want || (row.ends_with('…') && !shown.is_empty() && want.starts_with(&shown))
}

/// Rows from which the confirm page's quote may be cut short (2.1.318 shows a 30-line prompt's
/// first 4 lines).
const QUOTE_CUT_ROWS: usize = 4;

/// The confirm page may quote this prompt: the whole of it, or its start when the quote fills the
/// rows it may be cut at.
pub fn rewind_quote_matches(quote: &[String], text: &str) -> bool {
    let shown = rewind_key(&quote.concat());
    let want = rewind_key(text);
    shown == want
        || (quote.len() >= QUOTE_CUT_ROWS && !shown.is_empty() && want.starts_with(&shown))
}

/// The seconds ago a confirm page's `5s ago` / `3m ago` / `2h ago` / `4d ago` stands for:
/// `[low, high)`. Other wording: `None`.
pub fn rewind_age(ago: &str) -> Option<(u64, u64)> {
    let n = ago.trim().strip_suffix(" ago")?;
    let unit = match n.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    let v: u64 = n[..n.len() - 1].parse().ok()?;
    Some((v * unit, (v + 1) * unit))
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
        assert_eq!(d.number_for(Meaning::Allow), Some(1));
        assert_eq!(d.number_for(Meaning::AllowAlways), Some(2));
        assert_eq!(d.number_for(Meaning::Deny), Some(4));
        assert_eq!(
            d.options[2].2,
            Meaning::Other,
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
        assert_eq!(d.number_for(Meaning::AllowAlways), Some(2));
        assert_eq!(d.number_for(Meaning::Deny), Some(3));
    }

    #[test]
    fn multi_line_command_and_typographic_apostrophe() {
        let lines = screen("permission_multiline_bash.txt");
        let d = permission(&lines).unwrap();
        assert_eq!(d.number_for(Meaning::AllowAlways), Some(2));
        assert_eq!(d.number_for(Meaning::Deny), Some(4));
        let command = "python3 -I -c \"\nfrom PIL import Image\nim = Image.open('/private/var";
        let needle: String = command
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(40)
            .collect();
        assert!(permission_body(&lines).unwrap().contains(&needle));
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn permission_mode_from_the_footer() {
        use crate::rpc::PermissionMode;
        let base = |footer: &str| lines(&["❯ hello", "─────", "❯", "─────", footer]);
        assert_eq!(
            permission_mode(&base("  ⏸ manual mode on · ? for shortcuts")),
            Some(PermissionMode::Manual)
        );
        assert_eq!(
            permission_mode(&base("  ⏵⏵ accept edits on (shift+tab to cycle)")),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            permission_mode(&base("  ⏸ plan mode on (shift+tab to cycle)")),
            Some(PermissionMode::Plan)
        );
        assert_eq!(
            permission_mode(&base("  ⏵⏵ auto mode on (shift+tab to cycle)")),
            Some(PermissionMode::Auto)
        );
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
        assert!(
            matches!(tasks_panel(&details), Some(TasksPanel::Details(t)) if t.contains("sleep 25"))
        );
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
        assert_eq!(d.number_for(Meaning::Allow), Some(1));
        assert_eq!(d.number_for(Meaning::Deny), Some(3));
        assert_eq!(d.number_for(Meaning::AllowAlways), None);
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

    #[test]
    fn agents_page_has_no_input_box() {
        // Its box starts a new background Session: never typed into.
        let list = screen("agents_list.txt");
        assert!(agents_page(&list));
        assert_eq!(input_box(&list), None);
        // The background Session it opens shows the usual input box and mode line.
        let bg = screen("background_session.txt");
        assert!(!agents_page(&bg));
        assert_eq!(input_box(&bg).as_deref(), Some(""));
        assert_eq!(
            permission_mode(&bg),
            Some(crate::rpc::PermissionMode::Manual)
        );
        assert!(!agents_page(&screen("idle_prompt.txt")));
    }

    /// The agents page's hints change with the selection and once a task is typed into its box;
    /// a person's draft there is never taken for the input box (ctrl+u would erase it).
    #[test]
    fn agents_page_with_other_hints_or_a_draft() {
        let list = screen("agents_list.txt");
        let with = |box_line: &str, hints: &str, keep_anchors: bool| -> Vec<String> {
            let mut l = list.clone();
            let n = l.len();
            l[n - 3] = box_line.into();
            l[n - 1] = hints.into();
            if !keep_anchors {
                for row in l.iter_mut().take(n - 4) {
                    row.clear();
                }
                l[10] = "Working 1".into();
            }
            l
        };
        let stop = with(
            "❯ describe a task for a new session",
            "  enter to open · ctrl+x to stop · ? for shortcuts",
            true,
        );
        let draft = with(
            "❯ fix the flaky test",
            "  ctrl+j for newline · ctrl+enter to start and open",
            true,
        );
        // Hints not seen before, and only a folded section heading left on screen.
        let unseen = with(
            "❯ fix the flaky test",
            "  enter to send · esc to clear",
            false,
        );
        for l in [&stop, &draft, &unseen] {
            assert!(agents_page(l), "{l:#?}");
            assert_eq!(input_box(l), None);
        }
        // Moved to the background: only a box with the mode line under it is the conversation's.
        let scr = |lines: Vec<String>| crate::screen::Screen {
            lines,
            moved: true,
            ..Default::default()
        };
        let mut odd = screen("background_session.txt");
        let n = odd.len();
        odd[n - 1] = "  enter to send · esc to clear".into();
        assert_eq!(input_text(&scr(odd)), None);
        assert_eq!(
            input_text(&scr(screen("background_session.txt"))).as_deref(),
            Some("")
        );
        // Not moved: the usual input box.
        assert_eq!(
            input_text(&crate::screen::Screen {
                lines: screen("idle_prompt.txt"),
                ..Default::default()
            })
            .as_deref(),
            Some("")
        );
    }

    #[test]
    fn ask_in_a_quote_bar() {
        // Claude Code 2.1.318 draws the question in a `│ ` bar; CJK text wraps without a space.
        let Some(AskScreen::Question(q)) = ask(&screen("ask_single_quoted.txt")) else {
            panic!()
        };
        let asked = "服务部署在 notes.example.com，用它自带的账号登录，再接 SSO。首次初始化向导不需要鉴权，所以先只开放内网。访问范围怎么定？";
        assert!(same_text(&q.title, asked), "{}", q.title);
        assert!(!q.tabbed);
        let labels: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, vec!["内外网都可访问 (Recommended)", "仅内网"]);
        assert_eq!(q.other, Some(3));
    }

    #[test]
    fn input_box_under_a_titled_rule() {
        // A Session named with `/rename` shows its name in the top rule (Claude Code 2.1.318).
        let rule = "─".repeat(60);
        let lines: Vec<String> = [
            "❯ /rename probe-a".to_string(),
            "  ⎿  Session renamed to: probe-a".to_string(),
            format!("{} probe-a ─", "─".repeat(50)),
            "❯ /cost".to_string(),
            rule.clone(),
            "  ⏵⏵ bypass permissions on (shift+tab to cycle)".to_string(),
        ]
        .into();
        assert_eq!(input_box(&lines).as_deref(), Some("/cost"));
        assert_eq!(
            permission_mode(&lines),
            Some(crate::rpc::PermissionMode::Bypass)
        );
        assert!(!is_input_rule(&format!(
            "{} Settings {}",
            "─".repeat(30),
            "─".repeat(30)
        )));
        assert!(!is_input_rule("── probe-a ─"));
        // A long name on a narrow screen still leaves a short rule before it.
        assert!(is_input_rule(&format!(
            "{} {} ─",
            "─".repeat(6),
            "a-rather-long-session-name-for-a-narrow-pane"
        )));
        assert!(!is_input_rule(&format!("{} a ─ b ─", "─".repeat(30))));
    }

    #[test]
    fn input_text_skips_the_dim_placeholder() {
        let box_ = |inner: &str| {
            let rule = "─".repeat(30);
            let b = format!("{rule}\r\n\x1b[0m❯\u{a0}{inner}\x1b[0m\r\n{rule}");
            crate::screen::Screen::render(b.as_bytes(), 30, 3)
        };
        let s = box_("\x1b[0;2mTry \"edit <filepath> to...\"");
        assert_eq!(
            input_box(&s.lines).as_deref(),
            Some("Try \"edit <filepath> to...\"")
        );
        assert_eq!(input_text(&s).as_deref(), Some(""));
        assert_eq!(input_text(&box_("hello")).as_deref(), Some("hello"));
    }
    #[test]
    fn feedback_draft_card_above_the_input_box() {
        // 2.1.318 at 80 columns (the input box pushed off a 24-row screen).
        let card = lines(&[
            "✻ Cogitated for 14s · done 6:54 PM",
            "",
            "╭──────────────────────────────────────────────────────────────────────────────╮",
            "│ ✻ Product feedback drafted: ccrpc 界面识别测试，请勿处理                     │",
            "│ │ - What happened: 这是一次本地界面测试，没有真实问题。用户要求模型起草一条  │",
            "│ │ - Repro: 在 …                                                              │",
            "│ 1 to review · 2 to send · 0 to dismiss                                       │",
            "╰──────────────────────────────────────────────────────────────────────────────╯",
            "",
        ]);
        assert_eq!(
            feedback_draft(&card).map(|c| (c.title, c.bottom)),
            Some(("ccrpc 界面识别测试，请勿处理".to_string(), 7))
        );
        // With the effort mark and the input box below it (160 columns).
        let mut wide = card.clone();
        wide.extend(lines(&[
            "                                ◉ xhigh · /effort",
            "─────",
            "❯",
            "─────",
            "  ⏵⏵ bypass permissions on · 1 feedback draft",
        ]));
        assert!(feedback_draft(&wide).is_some());
        // Quoted in the conversation: indented, or more conversation below it.
        let quoted: Vec<String> = card.iter().map(|l| format!("  {l}")).collect();
        assert!(feedback_draft(&quoted).is_none());
        let mut above_reply = card.clone();
        above_reply.push("⏺ 这是卡片的样子。".into());
        assert!(feedback_draft(&above_reply).is_none());
        // More drafts waiting behind it.
        let mut queued = card.clone();
        queued[6] =
            "│ 1 to review · 2 to send · 0 to dismiss · +1 more queued                       │"
                .into();
        assert!(feedback_draft(&queued).is_some());
        // Without its key line it is not the card (the keys would mean something else).
        let mut no_keys = card.clone();
        no_keys[6] =
            "│ 1 to review · 2 to send                                                      │"
                .into();
        assert!(feedback_draft(&no_keys).is_none());
        assert!(feedback_draft(&lines(&["❯ 1 to review · 2 to send · 0 to dismiss"])).is_none());
    }

    #[test]
    fn empty_input_box_below_the_card() {
        use crate::screen::Screen;
        let screen = |rows: &[&str], cursor: (u16, u16)| Screen {
            lines: lines(rows),
            undimmed: lines(rows),
            cursor,
            ..Default::default()
        };
        let base = ["╰────╯", "", "─────", "❯", "─────"];
        assert_eq!(input_empty_below(&screen(&base, (3, 2)), 0), Some(true));
        // Something typed; a typed space hidden by trimming (the cursor moved on); two rows.
        let typed = ["╰────╯", "", "─────", "❯ 2", "─────"];
        assert_eq!(input_empty_below(&screen(&typed, (3, 4)), 0), Some(false));
        assert_eq!(input_empty_below(&screen(&base, (3, 3)), 0), Some(false));
        let two = ["╰────╯", "─────", "❯", "", "─────"];
        assert_eq!(input_empty_below(&screen(&two, (2, 2)), 0), Some(false));
        // No input box under the card (a short screen).
        assert_eq!(
            input_empty_below(&screen(&["╰────╯", "", ""], (2, 0)), 0),
            None
        );
    }

    #[test]
    fn trust_folder_dialog() {
        let lines = screen("trust_folder.txt");
        assert_eq!(trust_folder(&lines), Some(false), "the cursor starts on No");
        let moved: Vec<String> = lines
            .iter()
            .map(|l| match l.trim() {
                "❯ No, exit" => "   No, exit".to_string(),
                "Yes, I trust this folder" => " ❯ Yes, I trust this folder".to_string(),
                _ => l.clone(),
            })
            .collect();
        assert_eq!(trust_folder(&moved), Some(true));
        assert_eq!(trust_folder(&screen("idle_prompt.txt")), None);
    }

    #[test]
    fn exit_question_with_tasks_running() {
        let lines = screen("exit_with_tasks.txt");
        assert_eq!(
            exit_with_tasks(&lines),
            Some(true),
            "the cursor starts on it"
        );
        let moved: Vec<String> = lines
            .iter()
            .map(|l| match l.trim() {
                "❯ 1. Exit and stop tasks" => "     1. Exit and stop tasks".to_string(),
                "2. Move to background and exit" => {
                    "   ❯ 2. Move to background and exit".to_string()
                }
                _ => l.clone(),
            })
            .collect();
        assert_eq!(exit_with_tasks(&moved), Some(false));
        // Not the known question: no title, no option of that name, or no footer.
        let without = |drop: &str| -> Vec<String> {
            lines.iter().filter(|l| l.trim() != drop).cloned().collect()
        };
        assert_eq!(
            exit_with_tasks(&without("Background work is running")),
            None
        );
        assert_eq!(
            exit_with_tasks(&without("Enter to confirm · Esc to cancel")),
            None
        );
        let renamed: Vec<String> = lines
            .iter()
            .map(|l| l.replace("Exit and stop tasks", "Exit"))
            .collect();
        assert_eq!(exit_with_tasks(&renamed), None);
        assert_eq!(exit_with_tasks(&screen("idle_prompt.txt")), None);
        assert_eq!(exit_with_tasks(&screen("trust_folder.txt")), None);
    }

    #[test]
    fn spinner_line() {
        let s = spinner(&screen("busy_spinner.txt")).unwrap();
        assert_eq!(
            (s.verb.as_str(), s.elapsed_s, s.tokens),
            ("Deliberating", Some(4), Some(6))
        );
        let l = |t: &str| vec![t.to_string()];
        let s = spinner(&l(
            "✶ Brewing… (running PreToolUse hook · 1m 2s · ↓ 1,046 tokens)",
        ))
        .unwrap();
        assert_eq!((s.elapsed_s, s.tokens), (Some(62), Some(1046)));
        assert_eq!(
            spinner(&l("✻ Waiting for 1 background agent to finish")),
            None,
            "no …"
        );
        assert_eq!(spinner(&l("✻ Baked for 0s · done 10:35 PM")), None);
        assert_eq!(spinner(&screen("idle_prompt.txt")), None);
    }

    #[test]
    fn rewind_picker_rows_and_window() {
        let p = rewind_picker(&screen("rewind_picker_current.txt")).unwrap();
        assert_eq!(p.cursor(), Some(REWIND_CURRENT));
        assert_eq!((p.above, p.below), (8, 0));
        assert_eq!(p.position(), Some(11));
        let texts: Vec<&str> = p.rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            texts,
            ["steered text", "/compact", "after compact", "(current)"]
        );

        let p = rewind_picker(&screen("rewind_picker_middle.txt")).unwrap();
        assert_eq!(p.cursor(), Some("steered text"));
        assert_eq!((p.above, p.below), (5, 3));
        assert_eq!(p.position(), Some(8));
        assert_eq!(p.rows[0].0, "multi line 1…");

        let p = rewind_picker(&screen("rewind_picker_top.txt")).unwrap();
        assert_eq!(p.cursor(), Some("first plain"));
        assert_eq!((p.above, p.below), (0, 8));
        assert_eq!(p.position(), Some(0));
        // Not a picker: the confirm page, an idle screen.
        assert!(rewind_picker(&screen("rewind_confirm_code.txt")).is_none());
        assert!(rewind_picker(&screen("rewind_restored.txt")).is_none());
    }

    #[test]
    fn rewind_confirm_with_code_and_scrolled_menu() {
        let c = rewind_confirm(&screen("rewind_confirm_code.txt")).unwrap();
        assert_eq!(c.quote, ["MOCK-WRITE /tmp/proj/w2.txt code-only"]);
        assert_eq!(c.ago.as_deref(), Some("5s ago"));
        assert_eq!(
            c.code.as_deref(),
            Some("The code will be restored -1 in w2.txt.")
        );
        assert!(
            c.warning
                .as_deref()
                .unwrap()
                .starts_with("Restored files go back whole")
        );
        let labels: Vec<(u32, &str)> = c.options.iter().map(|o| (o.0, o.1.as_str())).collect();
        assert_eq!(
            labels,
            [
                (1, "Restore code and conversation"),
                (2, "Restore conversation"),
                (3, "Restore code"),
                (4, "Summarize from here"),
                (5, "Summarize up to here"),
            ]
        );
        assert!(c.options[0].2);
        assert!(rewind_confirm(&screen("rewind_picker_middle.txt")).is_none());
    }

    #[test]
    fn rewind_confirm_quotes_the_first_rows() {
        let c = rewind_confirm(&screen("rewind_confirm_multiline.txt")).unwrap();
        assert_eq!(c.quote.len(), 4);
        assert_eq!(c.ago.as_deref(), Some("11s ago"));
        assert_eq!(c.code.as_deref(), Some("The code will be unchanged."));
        assert_eq!(c.warning, None);
        assert_eq!(c.options.len(), 4);
        let multi: Vec<String> = (1..=30).map(|i| format!("multi line {i}")).collect();
        assert!(rewind_quote_matches(&c.quote, &multi.join("\n")));
        assert!(!rewind_quote_matches(
            &c.quote,
            "multi line 2\nmulti line 1"
        ));

        // A long line wraps and its long word breaks.
        let c = rewind_confirm(&screen("rewind_confirm_wrapped.txt")).unwrap();
        let long = format!("long-single {}", "abcdefghij".repeat(30));
        assert!(rewind_quote_matches(&c.quote, &long));
        assert!(!rewind_quote_matches(&c.quote, "long-single abc"));

        // Images show as `[Image #N]` tags (numbered across the Session).
        let c = rewind_confirm(&screen("rewind_confirm_image.txt")).unwrap();
        assert!(rewind_quote_matches(&c.quote, "with image\nsecond line"));
    }

    #[test]
    fn rewind_rows_match_prompts() {
        assert!(rewind_row_matches("first plain", "first plain"));
        assert!(rewind_row_matches("leading spaces", "  leading spaces  "));
        assert!(rewind_row_matches(
            "multi line 1…",
            "multi line 1\nmulti line 2"
        ));
        assert!(rewind_row_matches(
            "[Image #2]with image…",
            "with image\nsecond line"
        ));
        assert!(rewind_row_matches("[Image #1]", ""));
        // A row cut at the screen's edge ends in `…`; a start without it is another prompt.
        let long = format!("long-single {}", "abcdefghij".repeat(30));
        assert!(rewind_row_matches(&format!("{}…", &long[..80]), &long));
        assert!(!rewind_row_matches(&long[..80], &long));
        assert!(!rewind_row_matches("first", "first plain"));
        assert!(!rewind_row_matches("continue", "continue later"));
        assert!(!rewind_row_matches("(current)", "first plain"));
        assert_eq!(rewind_key("[Image #12] a\tb [Image #x]"), "ab[Image#x]");
    }

    #[test]
    fn input_box_in_shell_mode() {
        let rule = "─".repeat(40);
        let screen = |row: &str| vec![rule.clone(), row.to_string(), rule.clone()];
        assert_eq!(input_box(&screen("❯ hello")).as_deref(), Some("hello"));
        assert_eq!(
            input_box(&screen("! echo bang")).as_deref(),
            Some("!echo bang")
        );
        assert_eq!(input_box(&screen("!")).as_deref(), Some("!"));
        assert_eq!(input_box(&screen("  something else")), None);
        // A restored `!` command fits its prompt text.
        assert_eq!(rewind_key("!echo bang"), rewind_key("! echo bang"));
        // Shell mode is told by the box's first column, not by the text.
        let scr = |row: &str| crate::screen::Screen {
            lines: screen(row),
            undimmed: Vec::new(),
            cursor: (0, 0),
            ..Default::default()
        };
        assert_eq!(
            shell_input(&scr("! echo bang")).as_deref(),
            Some("echo bang")
        );
        assert_eq!(shell_input(&scr("!")).as_deref(), Some(""));
        assert_eq!(shell_input(&scr("❯ !echo bang")), None);
    }

    #[test]
    fn rewind_quote_whole_unless_cut() {
        let q = |rows: &[&str]| rows.iter().map(|r| r.to_string()).collect::<Vec<_>>();
        // A short quote is the whole prompt: a longer prompt starting the same is another one.
        assert!(rewind_quote_matches(
            &q(&["fix the tests"]),
            "fix the tests"
        ));
        assert!(!rewind_quote_matches(
            &q(&["fix the tests"]),
            "fix the tests in parser"
        ));
        // Four rows may be cut short.
        assert!(rewind_quote_matches(
            &q(&["a", "b", "c", "d"]),
            "a\nb\nc\nd\ne"
        ));
        assert!(!rewind_quote_matches(
            &q(&["a", "b", "c", "x"]),
            "a\nb\nc\nd\ne"
        ));
        assert_eq!(rewind_age("5s ago"), Some((5, 6)));
        assert_eq!(rewind_age("3m ago"), Some((180, 240)));
        assert_eq!(rewind_age("2h ago"), Some((7200, 10800)));
        assert_eq!(rewind_age("yesterday"), None);
    }

    #[test]
    fn rewind_screens_need_their_frame() {
        // The words quoted in the conversation, with the input box below: not the screen.
        let mut lines = screen("rewind_confirm_code.txt");
        lines.extend(screen("rewind_restored.txt").into_iter().skip(43));
        assert!(rewind_confirm(&lines).is_none());
        // Without the dialog rule above the title: not the screen.
        let mut lines = screen("rewind_picker_middle.txt");
        let at = lines.iter().position(|l| l.trim() == "Rewind").unwrap();
        lines[at - 1] = String::new();
        assert!(rewind_picker(&lines).is_none());
        // A wrapped option continues its label.
        let mut lines = screen("rewind_confirm_code.txt");
        let at = lines
            .iter()
            .position(|l| l.contains("1. Restore code and conversation"))
            .unwrap();
        lines[at] = "   ❯ 1. Restore code".into();
        lines.insert(at + 1, "        and conversation".into());
        let c = rewind_confirm(&lines).unwrap();
        assert_eq!(c.options[0].1, "Restore code and conversation");
    }
}

/// The permission mode shown in the footer below the input box (`⏸ manual mode on`, `⏵⏵ accept
/// edits on`, `⏸ plan mode on`, `⏵⏵ auto mode on`).
pub fn permission_mode(lines: &[String]) -> Option<crate::rpc::PermissionMode> {
    use crate::rpc::PermissionMode::*;
    let rule = lines.iter().rposition(|l| is_input_rule(l))?;
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

/// The effort level marked above the input box's top right (`◉ xhigh · /effort`, `○ low ·
/// /effort`; also on a dialog's frame). None when the model has no effort levels or it is not
/// on screen.
pub fn effort(lines: &[String]) -> Option<String> {
    const LEVELS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "auto"];
    lines.iter().rev().find_map(|l| {
        let at = l.find("· /effort")?;
        let level = l[..at].split_whitespace().last()?.to_lowercase();
        LEVELS.contains(&level.as_str()).then_some(level)
    })
}

#[cfg(test)]
mod effort_tests {
    use super::effort;

    fn l(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_the_mark_above_the_input_box_or_on_a_dialog_frame() {
        let idle = l(&[
            "✻ Worked for 0s · done 7:20 PM",
            "                                                       ◉ xhigh · /effort",
            "────────────────────────────────────────",
            "❯",
        ]);
        assert_eq!(effort(&idle).as_deref(), Some("xhigh"));
        let dialog = l(&["▔▔▔▔▔▔▔▔▔▔▔▔ ○ low · /effort ▔", "   Change effort level?"]);
        assert_eq!(effort(&dialog).as_deref(), Some("low"));
        // A model without effort levels shows no mark; prose mentioning /effort is not one.
        assert_eq!(effort(&l(&["❯", "  ⏸ manual mode on"])), None);
        assert_eq!(effort(&l(&["use the · /effort command"])), None);
    }
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
    let hint = lines.iter().rposition(|l| {
        l.contains("x to stop")
            || l.contains("Esc to close")
            || l.contains("Esc/Enter/Space to close")
    })?;
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
        v.iter()
            .enumerate()
            .map(|(i, s)| (s.to_string(), i == 0))
            .collect()
    }

    #[test]
    fn exactly_one_running_row() {
        let r = rows(&[
            "Shells (2)",
            "⏺ npm run dev   running",
            "⏺ sleep 300   running",
            "Local agents (1)",
            "◯ wait for deploy   running · m",
        ]);
        assert_eq!(task_row(&r, "sleep 300"), Ok(2));
        assert_eq!(task_row(&r, "wait for deploy"), Ok(4));
        // Only a whole leading match: `dev` is not `npm run dev`.
        assert!(task_row(&r, "dev").is_err());
        assert!(task_row(&r, "sleep 9").is_err());
        let two = rows(&["⏺ npm run dev   running", "⏺ npm run dev   running"]);
        assert!(task_row(&two, "npm run dev").is_err());
        // The same command already ended: the target may be that one.
        let ended = rows(&[
            "⏺ sleep 300   running",
            "Completed (1)",
            "✔ sleep 300   done",
        ]);
        assert!(task_row(&ended, "sleep 300").is_err());
    }
}
