//! SQLite index: a disposable mirror cache in the app's Caches folder (ADR-0003).
//! Opened with WAL and `synchronous=NORMAL`; rebuilt (never migrated) when
//! `PRAGMA user_version` or `meta.data_root` differ.

pub mod reader;
pub mod schema;
pub mod text;
pub mod writer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension};

use crate::error::CoreResult;
use crate::pricing::{PriceTable, Usage};
use schema::{DDL, DROP_ALL, SCHEMA_VERSION};

const READ_POOL_MAX: usize = 4;

/// Writer connection + read pool.
#[derive(Debug)]
pub struct Index {
    pub path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
}

impl Index {
    /// Opens (or recreates) the index at `path` for `data_root`. A schema-version or data-root
    /// mismatch, or an unreadable file, deletes `index.sqlite*` and starts over (ADR-0003).
    /// `prices` backs the `cv_cost` SQL function on the writer connection.
    pub fn open(
        path: &Path,
        data_root: &Path,
        prices: Arc<RwLock<PriceTable>>,
    ) -> CoreResult<Self> {
        let root = data_root.display().to_string();
        let conn = match open_valid(path, &root) {
            Ok(Some(conn)) => conn,
            Ok(None) | Err(_) => {
                delete_db_files(path);
                let conn = open_conn(path)?;
                create(&conn, &root)?;
                conn
            }
        };
        register_cost_fn(&conn, prices)?;
        Ok(Index {
            path: path.to_path_buf(),
            writer: Mutex::new(conn),
            readers: Mutex::new(Vec::new()),
        })
    }

    /// Runs `f` with the (single) writer connection.
    pub fn write<T>(&self, f: impl FnOnce(&mut Connection) -> CoreResult<T>) -> CoreResult<T> {
        let mut conn = self.writer.lock();
        f(&mut conn)
    }

    /// Runs `f` with a pooled read connection.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> CoreResult<T>) -> CoreResult<T> {
        let pooled = self.readers.lock().pop();
        let conn = match pooled {
            Some(c) => c,
            None => open_conn(&self.path)?,
        };
        let out = f(&conn);
        let mut pool = self.readers.lock();
        if pool.len() < READ_POOL_MAX {
            pool.push(conn);
        }
        out
    }

    /// Drops every table and recreates the schema in place (readers keep working).
    pub fn reset(&self) -> CoreResult<()> {
        self.write(|conn| {
            let root: String = conn
                .query_row("SELECT value FROM meta WHERE key='data_root'", [], |r| {
                    r.get(0)
                })
                .optional()?
                .unwrap_or_default();
            conn.execute_batch(&format!("BEGIN; {DROP_ALL} COMMIT;"))?;
            create(conn, &root)?;
            conn.execute_batch("VACUUM;")?;
            Ok(())
        })
    }
}

/// See [`writer::COST_EXPR`].
fn register_cost_fn(conn: &Connection, prices: Arc<RwLock<PriceTable>>) -> CoreResult<()> {
    conn.create_scalar_function("cv_cost", 8, FunctionFlags::SQLITE_UTF8, move |ctx| {
        let model: Option<String> = ctx.get(0)?;
        let tok = |i| ctx.get::<i64>(i).map(|v| v.max(0) as f64);
        let fast = ctx.get::<i64>(6)? != 0;
        let usage = Usage::from_tokens(tok(1)?, tok(2)?, tok(3)?, tok(4)?, tok(5)?, fast);
        Ok(prices.read().cost(model.as_deref(), &usage, ctx.get(7)?))
    })?;
    Ok(())
}

fn open_conn(path: &Path) -> CoreResult<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

/// Opens an existing index and returns it only if its version and data root match.
fn open_valid(path: &Path, root: &str) -> CoreResult<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = open_conn(path)?;
    let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Ok(None);
    }
    let stored: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='data_root'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok((stored.as_deref() == Some(root)).then_some(conn))
}

fn create(conn: &Connection, root: &str) -> CoreResult<()> {
    conn.execute_batch(&format!("BEGIN; {DDL} COMMIT;"))?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('data_root', ?1)",
        [root],
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn delete_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}
