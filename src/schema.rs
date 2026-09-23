//! Known legacy schemas upgrade on first use, after a verified SQLite backup.
use crate::error::Error;
use rusqlite::{
    Connection, OpenFlags, TransactionBehavior,
    backup::{Backup, StepResult},
};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::{fs, io};

pub const SCHEMA_VERSION: i64 = 3;
const PEERS: &str = "CREATE TABLE peers (
    name TEXT PRIMARY KEY, harness TEXT NOT NULL,
    session_id TEXT, workspace TEXT, socket TEXT, url TEXT,
    delivery TEXT NOT NULL DEFAULT 'native' CHECK(delivery IN ('native', 'pull')),
    UNIQUE(harness, session_id),
    CHECK ((delivery='native' AND session_id IS NOT NULL AND workspace IS NOT NULL)
        OR (delivery='pull' AND session_id IS NULL AND workspace IS NULL
            AND socket IS NULL AND url IS NULL)))";

fn version(db: &Connection) -> Result<i64, Error> {
    Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn has_column(db: &Connection, table: &str, name: &str) -> Result<bool, Error> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name=?)",
        [table, name],
        |r| r.get(0),
    )?)
}

fn check_version(version: i64) -> Result<(), Error> {
    match version {
        0..=2 | SCHEMA_VERSION => Ok(()),
        _ => Err(Error::code("unsupported_database_version")),
    }
}

pub(crate) fn backup_directory(path: &Path) -> PathBuf {
    let mut directory = path.as_os_str().to_os_string();
    directory.push(".backups");
    directory.into()
}

fn integrity(db: &Connection) -> Result<(), Error> {
    let result: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if result != "ok" {
        return Err(Error::code("database_integrity_check_failed"));
    }
    Ok(())
}

/// The caller holds BEGIN IMMEDIATE on the source until migration commits. A
/// second read connection can copy the committed state without a write gap.
fn backup(path: &Path, previous: i64) -> Result<(), Error> {
    let directory = backup_directory(path);
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() => (),
        Err(e) => return Err(e.into()),
    }
    let id = uuid::Uuid::new_v4();
    let pending = directory.join(format!(".pending-{id}"));
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&pending)?;
    struct Pending(PathBuf);
    impl Drop for Pending {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _pending = Pending(pending.clone());
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut destination = Connection::open(&pending)?;
    {
        let copy = Backup::new(&source, &mut destination)?;
        loop {
            if crate::os::interrupted() {
                return Err(Error::Interrupted);
            }
            match copy.step(128)? {
                StepResult::Done => break,
                StepResult::More => (),
                _ => return Err(Error::code("database_backup_failed")),
            }
        }
    }
    destination.execute_batch("PRAGMA journal_mode=DELETE")?;
    if version(&destination)? != previous {
        return Err(Error::code("database_backup_failed"));
    }
    integrity(&destination)?;
    destination.close().map_err(|(_, error)| error)?;
    file.sync_all()?;
    fs::rename(
        &pending,
        directory.join(format!(
            "schema-{previous}-before-{SCHEMA_VERSION}-{id}.sqlite3"
        )),
    )?;
    fs::File::open(&directory)?.sync_all()?;
    // Persist a newly created backup directory before the source can change.
    if let Some(parent) = directory.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn ensure(db: &mut Connection, path: &Path) -> Result<(), Error> {
    // Current databases need no writer lock. All checks use the same snapshot.
    {
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let v = version(&tx)?;
        check_version(v)?;
        if v == SCHEMA_VERSION {
            let ready: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='waits')
                 AND EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='retired_peers')",
                [], |r| r.get(0),
            )?;
            if !ready
                || !has_column(&tx, "peers", "delivery")?
                || !has_column(&tx, "messages", "wait_returned_at")?
            {
                return Err(Error::code("invalid_database_schema"));
            }
            tx.commit()?;
            return Ok(());
        }
    }
    // Rebuilding the referenced parent table requires foreign_keys=OFF outside
    // the transaction. Check all references before commit; restore enforcement
    // on both success and rollback. Never rename the old parent table.
    db.execute_batch("PRAGMA foreign_keys=OFF")?;
    let result = change(db, path);
    db.execute_batch("PRAGMA foreign_keys=ON")?;
    result
}

fn change(db: &mut Connection, path: &Path) -> Result<(), Error> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let v = version(&tx)?;
    check_version(v)?;
    if v == SCHEMA_VERSION {
        // Another initializer/migrator finished first.
        tx.commit()?;
        return Ok(());
    }
    if v == 0 {
        let nonempty: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
            [],
            |r| r.get(0),
        )?;
        if nonempty {
            return Err(Error::code("unsupported_unversioned_database"));
        }
        tx.execute_batch(PEERS)?;
        tx.execute_batch(
            "CREATE TABLE messages (
            seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
            sender TEXT NOT NULL REFERENCES peers(name),
            recipient TEXT NOT NULL REFERENCES peers(name),
            in_reply_to TEXT UNIQUE REFERENCES messages(id),
            body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
            submission TEXT NOT NULL CHECK(submission IN
                ('not_submitted', 'submission_unknown', 'submitted')),
            notification_started_at REAL, notification_finished_at REAL,
            notification_detail TEXT, wait_returned_at REAL)",
        )?;
    } else {
        backup(path, v).map_err(|error| match error {
            Error::Interrupted => error,
            _ => Error::code("database_backup_failed"),
        })?;
        if !has_column(&tx, "peers", "url")? {
            tx.execute_batch("ALTER TABLE peers ADD COLUMN url TEXT")?;
        }
        tx.execute_batch(&PEERS.replacen("CREATE TABLE peers", "CREATE TABLE peers_next", 1))?;
        tx.execute_batch(
            "INSERT INTO peers_next
            (name, harness, session_id, workspace, socket, url, delivery)
            SELECT name, harness, session_id, workspace, socket, url, 'native' FROM peers;
            DROP TABLE peers;
            ALTER TABLE peers_next RENAME TO peers;",
        )?;
        if !has_column(&tx, "messages", "wait_returned_at")? {
            tx.execute_batch("ALTER TABLE messages ADD COLUMN wait_returned_at REAL")?;
        }
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS waits (
        token TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id),
        actor TEXT NOT NULL, until REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS retired_peers (
        name TEXT PRIMARY KEY REFERENCES peers(name), retired_at REAL NOT NULL)",
    )?;
    if tx
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(Error::code("database_foreign_key_violation"));
    }
    if v != 0 {
        integrity(&tx)?;
    }
    if crate::os::interrupted() {
        return Err(Error::Interrupted);
    }
    tx.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}
