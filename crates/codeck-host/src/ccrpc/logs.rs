//! Per-connection logs: `~/Library/Logs/dev.joy.codeck/ccrpc/<date>/<holder_id>-<time>/` with
//! `wire.jsonl` (every line to and from ccrpc), `ccrpc.log` (ccrpc's own `--log`) and `gui.log`.
//! Owner-only; date directories older than 14 days are removed at startup.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// `wire.jsonl` is rotated to `wire.1.jsonl` past this size.
pub const WIRE_MAX: u64 = 50 * 1024 * 1024;
pub const KEEP_DAYS: i64 = 14;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn mkdir(p: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(p)
}

pub fn append(p: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)
}

/// A fresh directory for one connection: `<root>/<YYYY-MM-DD>/<name>-<HHMMSS>[-n]`.
pub fn conn_dir(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    let now = chrono::Local::now();
    let day = root.join(now.format("%Y-%m-%d").to_string());
    let base = format!("{name}-{}", now.format("%H%M%S"));
    mkdir(&day)?;
    // The leaf is created on its own (not recursively): two connections never share it.
    for n in 0.. {
        let dir = if n == 0 {
            day.join(&base)
        } else {
            day.join(format!("{base}-{n}"))
        };
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Removes `<root>/<date>` directories older than `KEEP_DAYS` (only names that are dates), except those holding a
/// directory of `live` (a connection still writing there).
pub fn prune(root: &Path, today: chrono::NaiveDate, live: &[PathBuf]) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(d) = name
            .to_str()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let path = e.path();
        if (today - d).num_days() > KEEP_DAYS
            && path.is_dir()
            && !live.iter().any(|l| l.starts_with(&path))
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Removes regular files directly in `dir` last modified more than `KEEP_DAYS` days ago (pasted
/// images: a prompt's image is read when it is sent).
pub fn prune_files(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let keep = std::time::Duration::from_secs(KEEP_DAYS as u64 * 24 * 3600);
    for e in rd.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if m.is_file()
            && m.modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > keep)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// `wire.jsonl`: `{"t": <unix ms>, "dir": "send" | "recv", "msg": <the line>}` per line (a line
/// that is not JSON is kept as a string).
pub struct Wire {
    path: PathBuf,
    file: File,
    size: u64,
    max: u64,
}

impl Wire {
    pub fn open(path: PathBuf) -> std::io::Result<Wire> {
        Wire::with_max(path, WIRE_MAX)
    }

    pub fn with_max(path: PathBuf, max: u64) -> std::io::Result<Wire> {
        let file = append(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Wire {
            path,
            file,
            size,
            max,
        })
    }

    /// Moves the directory the file is in (a new Session's directory gets its Holder ID).
    pub fn moved(&mut self, dir: &Path) {
        if let Some(name) = self.path.file_name() {
            self.path = dir.join(name);
        }
    }

    pub fn write(&mut self, dir: &str, line: &str) {
        let msg = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(_) => line.trim().to_string(),
            Err(_) => serde_json::Value::String(line.to_string()).to_string(),
        };
        let row = format!("{{\"t\":{},\"dir\":\"{dir}\",\"msg\":{msg}}}\n", now_ms());
        if self.size + row.len() as u64 > self.max && self.size > 0 {
            self.rotate();
        }
        if self.file.write_all(row.as_bytes()).is_ok() {
            self.size += row.len() as u64;
        }
    }

    fn rotate(&mut self) {
        let old = self.path.with_extension("1.jsonl");
        let _ = std::fs::rename(&self.path, old);
        if let Ok(f) = append(&self.path) {
            self.file = f;
            self.size = 0;
        }
    }
}

/// One `gui.log` line: `<unix seconds> <text>`.
pub fn line(f: &mut File, text: &str) {
    let t = now_ms() as f64 / 1000.0;
    let _ = writeln!(f, "{t:.3} {text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_rows_keep_json_and_quote_other_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("wire.jsonl");
        let mut w = Wire::open(p.clone()).unwrap();
        w.write("send", r#"{"type":"prompt","text":"hi"}"#);
        w.write("recv", "not json");
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows[0]["dir"], "send");
        assert_eq!(rows[0]["msg"]["text"], "hi");
        assert!(rows[0]["t"].as_u64().unwrap() > 0);
        assert_eq!(rows[1]["msg"], "not json");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn wire_rotates_past_its_size() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("wire.jsonl");
        let mut w = Wire::with_max(p.clone(), 200).unwrap();
        for i in 0..10 {
            w.write("recv", &format!(r#"{{"type":"state","n":{i}}}"#));
        }
        let cur = std::fs::read_to_string(&p).unwrap();
        let old = std::fs::read_to_string(d.path().join("wire.1.jsonl")).unwrap();
        assert!(cur.len() <= 200 && !cur.is_empty());
        assert!(cur.contains(r#""n":9"#));
        assert!(!old.is_empty());
    }

    #[test]
    fn prune_removes_only_old_date_dirs() {
        let d = tempfile::tempdir().unwrap();
        for n in ["2026-10-01", "2026-10-02", "2026-10-25", "notes"] {
            std::fs::create_dir(d.path().join(n)).unwrap();
        }
        prune(
            d.path(),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            &[d.path().join("2026-10-02/m1")],
        );
        assert!(!d.path().join("2026-10-01").exists());
        assert!(
            d.path().join("2026-10-02").exists(),
            "a live connection's day stays"
        );
        assert!(d.path().join("2026-10-25").exists());
        assert!(d.path().join("notes").exists());
    }

    #[test]
    fn prune_files_removes_only_old_regular_files() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = (d.path().join("1-1.png"), d.path().join("2-2.png"));
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&new, b"x").unwrap();
        let long_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        prune_files(d.path());
        assert!(!old.exists());
        assert!(new.exists());
        assert!(d.path().join("sub").exists());
    }
}
