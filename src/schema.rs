//! Schema changes are explicit. Ordinary opens never upgrade an existing mailbox.
use crate::error::Error;
use rusqlite::{Connection, TransactionBehavior};

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

fn check_version(version: i64, upgrade: bool) -> Result<(), Error> {
    match version {
        0 | SCHEMA_VERSION => Ok(()),
        1 | 2 if upgrade => Ok(()),
        1 | 2 => Err(Error::code("database_migration_required")),
        _ => Err(Error::code("unsupported_database_version")),
    }
}

pub fn ensure(db: &mut Connection, upgrade: bool) -> Result<(), Error> {
    // Current databases need no writer lock. All checks use the same snapshot.
    {
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let v = version(&tx)?;
        check_version(v, upgrade)?;
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
    let result = change(db, upgrade);
    db.execute_batch("PRAGMA foreign_keys=ON")?;
    result
}

fn change(db: &mut Connection, upgrade: bool) -> Result<(), Error> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let v = version(&tx)?;
    check_version(v, upgrade)?;
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
    if crate::os::interrupted() {
        return Err(Error::Interrupted);
    }
    tx.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}
