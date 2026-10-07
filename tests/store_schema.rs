//! Opening, creation, migration and per-connection settings of the shared database.
#[path = "store_support.rs"]
mod support;

use harness_talk::{
    error::Error,
    model::*,
    store::{self, Store},
};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use std::{fs, thread};
use support::*;

fn columns(db: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    stmt.query_map([], |r| r.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
fn version(db: &rusqlite::Connection) -> i64 {
    db.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_database_removed_after_opening_is_not_recreated() {
    let temp = Temp::new();
    Store::open(&temp.db(), true).unwrap();
    let store = Store::open(&temp.db(), false).unwrap();
    fs::remove_file(temp.db()).unwrap();
    assert!(matches!(store.peers(), Err(Error::Db(_))));
    assert!(!temp.db().exists());
}

#[test]
fn concurrent_first_opens_all_succeed() {
    let temp = Temp::new();
    let path = temp.path().join("new/mail.sqlite3");
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (path, barrier) = (path.clone(), barrier.clone());
            thread::spawn(move || {
                barrier.wait();
                Store::open(&path, true).map(|_| ())
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let db = raw(&path);
    assert_eq!(3, version(&db));
    assert_eq!(
        1,
        columns(&db, "messages")
            .iter()
            .filter(|c| *c == "wait_returned_at")
            .count()
    );
}

#[test]
fn busy_writer_stops_after_its_sleep_budget() {
    let temp = Temp::new();
    let store = store(&temp, &["alice"]);
    let holder = raw(&temp.db());
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let db = store.connect().unwrap();
    let started = Instant::now();
    let error = db.execute_batch("BEGIN IMMEDIATE").unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(
        error,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::DatabaseBusy
    ));
    assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
    // The budget counts requested sleep, so allow scheduler and I/O overhead. Linux sleeps
    // close to what is asked. A hosted macOS runner took 18.6 s for the same budget, so
    // elsewhere the bound only says that the wait ends.
    let allowed = if cfg!(target_os = "linux") { 10 } else { 60 };
    assert!(elapsed < Duration::from_secs(allowed), "{elapsed:?}");
}

#[test]
fn readonly_skip_check_never_creates_and_reports_only_codes() {
    let temp = Temp::new();
    let missing = temp.path().join("missing.sqlite3");
    assert_eq!(
        Err(Error::code("notification_state_unavailable")),
        store::skip_reason_readonly(&missing, "x", "bob")
    );
    assert!(!missing.exists());
    let store = store(&temp, &["alice", "bob"]);
    let request = store
        .save("alice", "bob", "Question", None, None)
        .unwrap()
        .0;
    assert_eq!(
        Ok(None),
        store::skip_reason_readonly(&temp.db(), &request.row.id, "bob")
    );
    assert_eq!(
        Err(Error::code("notification_message_not_found")),
        store::skip_reason_readonly(&temp.db(), &request.row.id, "alice")
    );
    store.ack(&request.row.id, "bob", &skipped).unwrap();
    assert_eq!(
        Ok(Some(SkipReason::AcknowledgedBeforeNotification)),
        store::skip_reason_readonly(&temp.db(), &request.row.id, "bob")
    );
    // Any sqlite failure, here a missing table, becomes the one fixed code.
    let other = store.save("alice", "bob", "Another", None, None).unwrap().0;
    raw(&temp.db())
        .execute_batch("DROP TABLE retired_peers")
        .unwrap();
    assert_eq!(
        Err(Error::code("notification_state_unavailable")),
        store::skip_reason_readonly(&temp.db(), &other.row.id, "bob")
    );
}

#[test]
fn current_schema_opens_and_reads_while_another_connection_holds_writer_lock() {
    let temp = Temp::new();
    let store = store(&temp, &["alice"]);
    let writer = store.connect().unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let reader = Store::open(&temp.db(), false).unwrap();
    assert_eq!("alice", reader.peers().unwrap()[0].name);
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn migration_preserves_receipts_replies_sequence_and_retirement() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let request = store
        .save("alice", "bob", "Question", None, None)
        .unwrap()
        .0;
    store
        .notify_once(
            &request.row.id,
            &|_, _| Outcome::unknown("uncertain"),
            &skipped,
        )
        .unwrap();
    let reply = store
        .save("bob", "alice", "Answer", None, Some(&request.row.id))
        .unwrap()
        .0;
    store.wait(&request.row.id, "alice", 0.0).unwrap();
    store.ack(&request.row.id, "bob", &skipped).unwrap();
    store.retire("bob").unwrap();
    raw(&temp.db())
        .execute(
            "INSERT INTO waits VALUES ('poll', ?, 'alice', 1234.5)",
            [&request.row.id],
        )
        .unwrap();
    let before = serde_json::to_value(store.get(&request.row.id, None).unwrap()).unwrap();
    let peers = store.peers().unwrap();
    schema_two(&temp.db());
    let migrated = Store::migrate(&temp.db()).unwrap();
    assert_eq!(
        before,
        serde_json::to_value(migrated.get(&request.row.id, None).unwrap()).unwrap()
    );
    assert_eq!(peers, migrated.peers().unwrap());
    assert_eq!(
        vec![(request.row.id, "alice".into(), 1234.5)],
        waits(&temp.db())
    );
    assert!(
        raw(&temp.db())
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
    migrated.restore("bob").unwrap();
    assert!(
        migrated
            .save("alice", "bob", "Next", None, None)
            .unwrap()
            .0
            .row
            .seq
            > reply.row.seq
    );
}

// These mirror the existing legacy compatibility fixtures. Malformed variants
// below are synthetic; no historical client is claimed to have produced them.
fn legacy_fixture(path: &std::path::Path, v: i64) {
    let db = raw(path);
    db.execute_batch(
        "CREATE TABLE peers (name TEXT PRIMARY KEY, harness TEXT NOT NULL,
        session_id TEXT NOT NULL, workspace TEXT NOT NULL, socket TEXT,
        UNIQUE(harness, session_id));
        CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT UNIQUE NOT NULL, sender TEXT NOT NULL REFERENCES peers(name),
        recipient TEXT NOT NULL REFERENCES peers(name), in_reply_to TEXT UNIQUE REFERENCES messages(id),
        body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
        submission TEXT NOT NULL CHECK(submission IN ('not_submitted', 'submission_unknown', 'submitted')),
        notification_started_at REAL, notification_finished_at REAL, notification_detail TEXT);
        INSERT INTO peers VALUES ('fixture', 'claude', 'fixture-session', '/tmp/fixture-workspace', NULL);
        INSERT INTO messages (id, sender, recipient, body, created_at, submission)
        VALUES ('fixture-message', 'fixture', 'fixture', 'Preserve this', 1.0, 'submission_unknown');",
    ).unwrap();
    if v == 2 {
        db.execute_batch("ALTER TABLE peers ADD COLUMN url TEXT")
            .unwrap();
    }
    db.execute_batch(&format!("PRAGMA user_version={v}"))
        .unwrap();
}

fn backup_dir(path: &std::path::Path) -> std::path::PathBuf {
    let mut directory = path.as_os_str().to_os_string();
    directory.push(".backups");
    directory.into()
}

fn backup_files(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let directory = backup_dir(path);
    if !directory.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|v| v == "sqlite3"))
        .collect();
    files.sort();
    files
}

#[test]
fn legacy_missing_socket_stops_repeated_opens_before_new_backups() {
    for v in [1, 2] {
        let temp = Temp::new();
        legacy_fixture(&temp.db(), v);
        raw(&temp.db())
            .execute_batch("ALTER TABLE peers DROP COLUMN socket")
            .unwrap();
        let before = fs::read(temp.db()).unwrap();
        let errors: Vec<_> = (0..2)
            .map(|_| Store::open(&temp.db(), false).unwrap_err().to_string())
            .collect();
        assert!(
            backup_files(&temp.db()).is_empty(),
            "repeated rejected opens created backups for schema {v}"
        );
        assert_eq!(
            errors,
            ["invalid_database_schema", "invalid_database_schema"]
        );
        assert_eq!(before, fs::read(temp.db()).unwrap());
        assert_eq!(v, version(&raw(&temp.db())));
    }
}

#[test]
fn malformed_legacy_shapes_preserve_existing_completed_backup() {
    for modification in [
        "ALTER TABLE messages DROP COLUMN body",
        "DROP TABLE messages",
        "CREATE TABLE waits (token TEXT PRIMARY KEY)",
        "CREATE TABLE retired_peers (name TEXT PRIMARY KEY)",
        "DROP TABLE peers; CREATE VIEW peers AS SELECT 'fixture' AS name, 'claude' AS harness,
         'fixture-session' AS session_id, '/tmp/fixture-workspace' AS workspace, NULL AS socket",
    ] {
        let temp = Temp::new();
        legacy_fixture(&temp.db(), 1);
        let directory = backup_dir(&temp.db());
        fs::create_dir(&directory).unwrap();
        let completed = directory.join("schema-1-before-3-preserved.sqlite3");
        fs::copy(temp.db(), &completed).unwrap();
        let backup_before = fs::read(&completed).unwrap();
        raw(&temp.db())
            .execute_batch(&format!("PRAGMA foreign_keys=OFF; {modification}"))
            .unwrap();
        let before = fs::read(temp.db()).unwrap();
        for _ in 0..2 {
            assert_eq!(
                "invalid_database_schema",
                code(Store::open(&temp.db(), false)),
                "{modification}"
            );
        }
        assert_eq!(vec![completed.clone()], backup_files(&temp.db()));
        assert_eq!(backup_before, fs::read(&completed).unwrap());
        assert_eq!(before, fs::read(temp.db()).unwrap());
        assert_eq!(1, version(&raw(&temp.db())));
    }
}

#[test]
fn valid_legacy_column_variants_migrate_once_with_verified_backup() {
    for v in [1, 2] {
        for optional in [false, true] {
            let temp = Temp::new();
            legacy_fixture(&temp.db(), v);
            if optional {
                raw(&temp.db()).execute_batch(
                    "ALTER TABLE messages ADD COLUMN wait_returned_at REAL;
                     ALTER TABLE messages ADD COLUMN extra TEXT;
                     ALTER TABLE peers ADD COLUMN extra TEXT;
                     CREATE TABLE waits (token TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id), actor TEXT NOT NULL, until REAL NOT NULL);
                     CREATE TABLE retired_peers (name TEXT PRIMARY KEY REFERENCES peers(name), retired_at REAL NOT NULL);").unwrap();
            }
            raw(&temp.db())
                .execute_batch("ALTER TABLE peers RENAME COLUMN socket TO SOCKET")
                .unwrap();
            let original_peers = columns(&raw(&temp.db()), "peers");
            let original_messages = columns(&raw(&temp.db()), "messages");
            Store::open(&temp.db(), false).unwrap();
            Store::open(&temp.db(), false).unwrap();
            let backups = backup_files(&temp.db());
            assert_eq!(1, backups.len());
            let backup = raw(&backups[0]);
            assert_eq!(v, version(&backup));
            assert_eq!(
                "ok",
                backup
                    .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                    .unwrap()
            );
            assert_eq!(original_peers, columns(&backup, "peers"));
            assert_eq!(original_messages, columns(&backup, "messages"));
            assert_eq!(
                "Preserve this",
                backup
                    .query_row(
                        "SELECT body FROM messages WHERE id='fixture-message'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap()
            );
            let migrated = raw(&temp.db());
            assert_eq!(3, version(&migrated));
            assert_eq!(
                "Preserve this",
                migrated
                    .query_row(
                        "SELECT body FROM messages WHERE id='fixture-message'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap()
            );
            assert_eq!(
                "native",
                migrated
                    .query_row("SELECT delivery FROM peers WHERE name='fixture'", [], |r| r
                        .get::<_, String>(0))
                    .unwrap()
            );
        }
    }
}
