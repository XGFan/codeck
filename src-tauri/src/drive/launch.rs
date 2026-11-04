//! Launch rules (「设置 › 驾驶 › 启动参数」): extra claude arguments for Sessions CoDeck starts in a directory
//! or below it, like a shell wrapper that adds `--add-dir` by working directory.

use std::collections::HashMap;
use std::path::Path;

use crate::settings::LaunchRule;

/// The arguments of the most specific rule whose directory is `cwd` or one of its ancestors (whole path
/// components: `/a/b` does not cover `/a/bc`), expanded with `env`; none when no rule covers it.
pub fn args_for(
    rules: &[LaunchRule],
    cwd: &Path,
    env: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let Some(rule) = rules
        .iter()
        .filter(|r| cwd.starts_with(&r.dir))
        .max_by_key(|r| Path::new(&r.dir).components().count())
    else {
        return Ok(Vec::new());
    };
    split(&rule.args, env).map_err(|e| format!("{} 的启动参数{e}", rule.dir))
}

/// What 「保存」 stores: trimmed, rules without arguments dropped. Fails on a relative or repeated directory
/// or arguments that do not split or expand with `env`.
pub fn normalize(
    rules: Vec<LaunchRule>,
    env: &HashMap<String, String>,
) -> Result<Vec<LaunchRule>, String> {
    let mut out: Vec<LaunchRule> = Vec::new();
    for r in rules {
        let dir = r.dir.trim().trim_end_matches('/').to_string();
        let args = r.args.trim().to_string();
        if args.is_empty() {
            continue;
        }
        if !Path::new(&dir).is_absolute() {
            return Err(format!("{dir} 不是绝对路径"));
        }
        if out.iter().any(|o| o.dir == dir) {
            return Err(format!("{dir} 重复了"));
        }
        split(&args, env).map_err(|e| format!("{dir} 的启动参数{e}"))?;
        out.push(LaunchRule { dir, args });
    }
    Ok(out)
}

/// Shell-style words: whitespace separates, `'…'` is literal, `"…"` and bare text expand `$VAR` / `${VAR}`
/// from `env`, `\` escapes the next character (in `"…"` only `"`, `\`, `$`), and an unquoted `~` at the start
/// of a word or right after its first `=` (`--add-dir=~/x`) becomes `$HOME` when followed by `/` or the end.
/// Errors (as the rest of a sentence) on an unpaired quote or a variable `env` does not have.
fn split(s: &str, env: &HashMap<String, String>) -> Result<Vec<String>, String> {
    let var = |name: &str| {
        env.get(name)
            .cloned()
            .ok_or_else(|| format!("里的环境变量 {name} 在登录 shell 里没有"))
    };
    let mut words = Vec::new();
    let mut cur: Option<String> = None;
    // Whether the current word has had an unquoted `=` yet (for `~` after the first one).
    let mut seen_eq = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(w) = cur.take() {
                    words.push(w);
                }
                seen_eq = false;
            }
            '\'' => {
                let w = cur.get_or_insert_with(String::new);
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(c) => w.push(c),
                        None => return Err("有没配对的引号".into()),
                    }
                }
            }
            '"' => {
                cur.get_or_insert_with(String::new);
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') if matches!(it.peek(), Some('"' | '\\' | '$')) => {
                            let n = it.next().unwrap();
                            cur.as_mut().unwrap().push(n);
                        }
                        Some('$') => {
                            let v = dollar(&mut it, &var)?;
                            cur.as_mut().unwrap().push_str(&v);
                        }
                        Some(c) => cur.as_mut().unwrap().push(c),
                        None => return Err("有没配对的引号".into()),
                    }
                }
            }
            '\\' => {
                let w = cur.get_or_insert_with(String::new);
                if let Some(n) = it.next() {
                    w.push(n);
                }
            }
            '$' => {
                let v = dollar(&mut it, &var)?;
                cur.get_or_insert_with(String::new).push_str(&v);
            }
            '~' if (cur.is_none()
                || (seen_eq && cur.as_deref().is_some_and(|w| w.ends_with('='))))
                && it.peek().is_none_or(|n| *n == '/' || n.is_whitespace()) =>
            {
                let home = var("HOME")?;
                cur.get_or_insert_with(String::new).push_str(&home);
            }
            '=' => {
                seen_eq = true;
                cur.get_or_insert_with(String::new).push('=');
            }
            c => cur.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(w) = cur {
        words.push(w);
    }
    Ok(words)
}

/// After a `$`: `NAME` or `{NAME}` expanded with `var`; a `$` not followed by a name stays a `$`.
fn dollar(
    it: &mut std::iter::Peekable<std::str::Chars>,
    var: &impl Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    let is_name = |c: char| c == '_' || c.is_ascii_alphanumeric();
    if it.peek() == Some(&'{') {
        it.next();
        let mut name = String::new();
        loop {
            match it.next() {
                Some('}') => break,
                Some(c) if is_name(c) => name.push(c),
                _ => return Err("里的 ${…} 没写完整".into()),
            }
        }
        return var(&name);
    }
    if !it
        .peek()
        .is_some_and(|c| *c == '_' || c.is_ascii_alphabetic())
    {
        return Ok("$".into());
    }
    let mut name = String::new();
    while let Some(&c) = it.peek() {
        if !is_name(c) {
            break;
        }
        name.push(c);
        it.next();
    }
    var(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(dir: &str, args: &str) -> LaunchRule {
        LaunchRule {
            dir: dir.into(),
            args: args.into(),
        }
    }

    fn env() -> HashMap<String, String> {
        HashMap::from([
            ("HOME".to_string(), "/Users/me".to_string()),
            ("EXTRA".to_string(), "/opt/extra".to_string()),
        ])
    }

    #[test]
    fn deepest_covering_directory_wins() {
        let rules = [
            rule("/Users/me/Developer", "--add-dir=$HOME/Developer/extra"),
            rule(
                "/Users/me/Developer/work",
                "--add-dir '/Users/me/work extra' --model sonnet",
            ),
        ];
        let at = |p: &str| args_for(&rules, Path::new(p), &env()).unwrap();
        assert_eq!(
            at("/Users/me/Developer"),
            ["--add-dir=/Users/me/Developer/extra"]
        );
        assert_eq!(
            at("/Users/me/Developer/app/src"),
            ["--add-dir=/Users/me/Developer/extra"]
        );
        assert_eq!(
            at("/Users/me/Developer/work/api"),
            ["--add-dir", "/Users/me/work extra", "--model", "sonnet"]
        );
        // Whole components only, and nothing outside every rule.
        assert!(at("/Users/me/Developer2").is_empty());
        assert!(at("/Users/me").is_empty());
    }

    #[test]
    fn expands_home_and_variables_like_a_shell() {
        let sp = |s: &str| split(s, &env()).unwrap();
        assert_eq!(
            sp("--add-dir=~/x ~ ~/y"),
            ["--add-dir=/Users/me/x", "/Users/me", "/Users/me/y"]
        );
        assert_eq!(
            sp("--add-dir ${HOME}/a \"$EXTRA/b c\""),
            ["--add-dir", "/Users/me/a", "/opt/extra/b c"]
        );
        // Literal: single quotes, escapes, `~` not at a word's start or after its `=`, `$` without a name.
        assert_eq!(
            sp("'$HOME/~' \\$HOME a~/b ~user cost$ \"\\$x\""),
            ["$HOME/~", "$HOME", "a~/b", "~user", "cost$", "$x"]
        );
        let err = |s: &str| split(s, &env()).unwrap_err();
        assert!(err("--add-dir=$NOPE/x").contains("NOPE"));
        assert!(err("--add-dir '/x").contains("引号"));
        assert!(err("${HOME").contains("没写完整"));
    }

    #[test]
    fn normalize_trims_drops_empty_and_rejects_bad_rules() {
        let e = env();
        let ok = normalize(
            vec![rule(" /a/b/ ", " --add-dir=/x "), rule("/c", "  ")],
            &e,
        )
        .unwrap();
        assert_eq!(ok, [rule("/a/b", "--add-dir=/x")]);
        assert!(
            normalize(vec![rule("a/b", "--x")], &e)
                .unwrap_err()
                .contains("不是绝对路径")
        );
        assert!(
            normalize(vec![rule("/a", "--x"), rule("/a/", "--y")], &e)
                .unwrap_err()
                .contains("重复")
        );
        assert!(
            normalize(vec![rule("/a", "--add-dir '/x")], &e)
                .unwrap_err()
                .contains("引号")
        );
        // Saved as typed, checked against the environment.
        assert_eq!(
            normalize(vec![rule("/a", "--add-dir=$HOME/x")], &e).unwrap(),
            [rule("/a", "--add-dir=$HOME/x")]
        );
        assert_eq!(
            normalize(vec![rule("/a", "--add-dir=$NOPE")], &e).unwrap_err(),
            "/a 的启动参数里的环境变量 NOPE 在登录 shell 里没有"
        );
    }
}
