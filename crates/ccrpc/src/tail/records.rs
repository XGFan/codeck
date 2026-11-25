//! What Claude Code's own records in the main file say: local settings, commands as typed.

use transcript::assemble::prompt;

/// What a `/model` or `/effort` output says it set: `Set model to \`Opus 4.7\` and saved …`,
/// `Kept model as Opus 4.7`, `Set model to Opus 4.7 (default) …`, `Set effort level to low (…`.
pub fn local_setting(s: &str) -> Option<(Option<String>, Option<String>)> {
    let t = s.trim();
    let t = t.strip_prefix("<local-command-stdout>").unwrap_or(t);
    let t = t
        .split("</local-command-stdout>")
        .next()
        .unwrap_or(t)
        .trim();
    if let Some(rest) = t
        .strip_prefix("Set model to ")
        .or_else(|| t.strip_prefix("Kept model as "))
    {
        let name = rest
            .split(" and saved")
            .next()
            .unwrap_or(rest)
            .lines()
            .next()
            .unwrap_or("");
        let name = name.replace('`', "");
        let name = name.trim().trim_end_matches("(default)").trim();
        return (!name.is_empty()).then(|| (Some(name.to_string()), None));
    }
    let rest = t.strip_prefix("Set effort level to ")?;
    let level: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (!level.is_empty()).then(|| (None, Some(level.to_lowercase())))
}

/// A slash command's record (`<command-name>/x</command-name>…<command-args>a</command-args>`) as
/// it was typed: `/x a`. Its output and `!` shell records are not commands.
/// A `!` command's record (`<bash-input>ls</bash-input>`) as `! ls`, as the TUI writes it.
pub(super) fn bash_line(s: &str) -> Option<String> {
    let t = s.trim_start();
    if !t.starts_with("<bash-input>") {
        return None;
    }
    Some(format!("! {}", prompt::tag(t, "bash-input")?.trim()))
}

pub(super) fn command_line(s: &str) -> Option<String> {
    let t = s.trim_start();
    if !t.starts_with("<command-name>") && !t.starts_with("<command-message>") {
        return None;
    }
    let name = prompt::tag(t, "command-name")?;
    let args = prompt::tag(t, "command-args").unwrap_or_default();
    let line = format!("/{} {}", name.trim().trim_start_matches('/'), args.trim());
    Some(line.trim_end().to_string())
}
