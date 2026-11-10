//! Flags are spelled out for each command; anything else starting with `--` is an error, so a
//! typo cannot be read as a search term. A single `-` is left alone: `-词` is search syntax.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::common::CliError;

pub struct Parsed {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
    pub positional: Vec<String>,
}

impl Parsed {
    pub fn parse(args: &[String], value_flags: &[&str], flags: &[&str]) -> Result<Self, CliError> {
        let mut out = Parsed {
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
            positional: Vec::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            i += 1;
            if value_flags.contains(&a.as_str()) {
                let Some(v) = args.get(i) else {
                    return Err(CliError::usage(format!("{a} 需要一个值")));
                };
                i += 1;
                out.values.insert(a.clone(), v.clone());
            } else if flags.contains(&a.as_str()) {
                out.flags.insert(a.clone());
            } else if a == "--" {
                out.positional.extend(args[i..].iter().cloned());
                break;
            } else if a.starts_with("--") {
                return Err(CliError::usage(format!("不认识的选项：{a}")));
            } else {
                out.positional.push(a.clone());
            }
        }
        Ok(out)
    }

    pub fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    pub fn flag(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }

    /// The value of `flag` as a number, `default` when it is not given.
    pub fn number(&self, flag: &str, default: Option<u64>) -> Result<Option<u64>, CliError> {
        match self.value(flag) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| CliError::usage(format!("{flag} 需要一个非负整数"))),
        }
    }

    /// The only positional argument, named `what` in the error when it is missing or repeated.
    pub fn one_positional(&self, what: &str) -> Result<&str, CliError> {
        match self.positional.as_slice() {
            [one] => Ok(one),
            _ => Err(CliError::usage(format!("需要一个{what}"))),
        }
    }
}

/// Removes `--cache-dir DIR` (a global option, anywhere on the line).
pub fn take_cache_dir(args: &mut Vec<String>) -> Result<Option<PathBuf>, CliError> {
    let Some(at) = args.iter().position(|a| a == "--cache-dir") else {
        return Ok(None);
    };
    if at + 1 >= args.len() {
        return Err(CliError::usage("--cache-dir 需要目录"));
    }
    let dir = args.remove(at + 1);
    args.remove(at);
    Ok(Some(PathBuf::from(dir)))
}
