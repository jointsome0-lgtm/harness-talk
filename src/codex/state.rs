//! The installed Codex CLI's saved thread addresses, read without starting a client.
use super::io_failure;
use crate::{error::Failure, os, validate::python_whitespace};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::Value;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

/// `$CODEX_HOME`, defaulting to `~/.codex`, resolved without requiring it to exist.
pub fn codex_home() -> PathBuf {
    let home = env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| os::home().join(".codex"));
    os::resolve(&os::expand_user(&home))
}

/// `state_5.sqlite` under the user-level `sqlite_home` setting, then `CODEX_SQLITE_HOME`, then `$CODEX_HOME`.
pub fn state_path() -> Result<PathBuf, Failure> {
    let home = codex_home();
    let configured = match read_config(&home.join("config.toml"))? {
        Some(config) => match config.get("sqlite_home") {
            Some(toml::Value::String(value)) if !value.is_empty() => Some(PathBuf::from(value)),
            Some(value) if truthy(value) => return Err(Failure::Class("TypeError")),
            _ => None,
        },
        None => None,
    };
    let state_home = match configured {
        Some(configured) => {
            let configured = os::expand_user(&configured);
            if configured.is_absolute() {
                configured
            } else {
                home.join(configured)
            }
        }
        None => {
            let variable = env::var_os("CODEX_SQLITE_HOME").unwrap_or_default();
            let variable = match variable.to_str() {
                Some(text) => PathBuf::from(text.trim_matches(python_whitespace)),
                None => PathBuf::from(variable),
            };
            if variable.as_os_str().is_empty() {
                home
            } else {
                os::expand_user(&variable)
            }
        }
    };
    Ok(os::resolve(&state_home.join("state_5.sqlite")))
}

/// The exact thread's `(id, cwd, archived, source)` row, opened read-only.
///
/// A non-text `id` becomes an empty string and a BLOB `cwd` or `source` becomes null, so
/// neither can match a registered address. A non-integer `archived` is 0 only for a zero REAL.
pub fn saved_thread(id: &str) -> Result<Option<(String, Value, i64, Value)>, Failure> {
    saved_thread_at(&state_path()?, id)
}

pub(crate) fn saved_thread_at(
    path: &Path,
    id: &str,
) -> Result<Option<(String, Value, i64, Value)>, Failure> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = Connection::open_with_flags(path, flags)?;
    db.busy_timeout(Duration::from_secs(3))?;
    let mut statement = db.prepare("SELECT id, cwd, archived, source FROM threads WHERE id=?")?;
    let mut rows = statement.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let text = |i: usize| -> Result<Value, Failure> {
        Ok(match row.get_ref(i)? {
            ValueRef::Null | ValueRef::Blob(_) => Value::Null,
            ValueRef::Integer(n) => Value::from(n),
            ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
            ValueRef::Text(bytes) => Value::String(
                String::from_utf8(bytes.to_vec())
                    .map_err(|_| Failure::Class("OperationalError"))?,
            ),
        })
    };
    let ident = match text(0)? {
        Value::String(ident) => ident,
        _ => String::new(),
    };
    let archived = match row.get_ref(2)? {
        ValueRef::Integer(n) => n,
        ValueRef::Real(0.0) => 0,
        _ => 1,
    };
    Ok(Some((ident, text(1)?, archived, text(3)?)))
}

fn read_config(path: &Path) -> Result<Option<toml::Table>, Failure> {
    // Like Path.exists(): a missing entry or a non-directory parent means no configuration.
    if let Err(error) = fs::metadata(path) {
        return match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP | libc::EBADF) => Ok(None),
            _ if error.kind() == io::ErrorKind::NotFound => Ok(None),
            _ => Err(io_failure(error)),
        };
    }
    let bytes = fs::read(path).map_err(io_failure)?;
    let text = String::from_utf8(bytes).map_err(|_| Failure::Class("UnicodeDecodeError"))?;
    text.parse::<toml::Table>()
        .map(Some)
        .map_err(|_| Failure::Class("TOMLDecodeError"))
}

fn truthy(value: &toml::Value) -> bool {
    match value {
        toml::Value::String(s) => !s.is_empty(),
        toml::Value::Integer(n) => *n != 0,
        toml::Value::Float(f) => *f != 0.0,
        toml::Value::Boolean(b) => *b,
        toml::Value::Datetime(_) => true,
        toml::Value::Array(items) => !items.is_empty(),
        toml::Value::Table(table) => !table.is_empty(),
    }
}
