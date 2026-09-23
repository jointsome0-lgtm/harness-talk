//! Opening, creation, migration and per-connection settings of the shared database.
#[path = "store_support.rs"]
mod support;

use harness_talk::{
    error::{Error, Failure},
    model::*,
    store::{self, Store},
};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier, mpsc};
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
fn creation_is_private_and_writes_schema_version_three() {
    let temp = Temp::new();
    let path = temp.path().join("a/b/mail.sqlite3");
    let store = Store::open(&path, true).unwrap();
    assert_eq!(path, store.path());
    assert_eq!(
        0o700,
        fs::metadata(temp.path().join("a/b"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    );
    assert_eq!(
        0o600,
        fs::metadata(&path).unwrap().permissions().mode() & 0o777
    );
    let db = raw(&path);
    assert_eq!(3, version(&db));
    assert_eq!(
        [
            "name",
            "harness",
            "session_id",
            "workspace",
            "socket",
            "url",
            "delivery"
        ],
        columns(&db, "peers")[..]
    );
    assert_eq!(
        Some("wait_returned_at"),
        columns(&db, "messages").last().map(String::as_str)
    );
    assert_eq!(
        ["token", "message_id", "actor", "until"],
        columns(&db, "waits")[..]
    );
    assert_eq!(["name", "retired_at"], columns(&db, "retired_peers")[..]);
}

#[test]
fn only_a_creating_open_makes_a_database() {
    let temp = Temp::new();
    let typo = temp.path().join("mistyped/mail.sqlite3");
    assert_eq!("database_not_found", code(Store::open(&typo, false)));
    assert!(!typo.parent().unwrap().exists());
    fs::write(temp.path().join("file"), "").unwrap();
    assert_eq!(
        "database_not_found",
        code(Store::open(&temp.path().join("file/mail.sqlite3"), false))
    );
    Store::open(&typo, true).unwrap();
    assert!(
        Store::open(&typo, false)
            .unwrap()
            .peers()
            .unwrap()
            .is_empty()
    );
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
fn corrupt_or_unreadable_databases_keep_their_own_errors() {
    let temp = Temp::new();
    let corrupt = temp.path().join("corrupt.sqlite3");
    fs::write(&corrupt, b"not a database".repeat(100)).unwrap();
    assert!(matches!(Store::open(&corrupt, false), Err(Error::Db(_))));
    assert!(matches!(Store::open(&corrupt, true), Err(Error::Db(_))));
    if unsafe { libc::geteuid() } != 0 {
        let locked = temp.path().join("locked");
        Store::open(&locked.join("mail.sqlite3"), true).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0)).unwrap();
        assert!(matches!(
            Store::open(&locked.join("mail.sqlite3"), false),
            Err(Error::Io(_))
        ));
    }
}

#[test]
fn unsupported_versions_are_refused_untouched() {
    let temp = Temp::new();
    Store::open(&temp.db(), true).unwrap();
    raw(&temp.db())
        .execute_batch("PRAGMA user_version=4")
        .unwrap();
    assert_eq!(
        "unsupported_database_version",
        code(Store::open(&temp.db(), false))
    );
    assert_eq!(4, version(&raw(&temp.db())));
}

#[test]
fn version_one_database_migrates_with_rows_intact() {
    let temp = Temp::new();
    let session = new_id();
    let request = new_id();
    raw(&temp.db()).execute_batch(&format!("
        CREATE TABLE peers (name TEXT PRIMARY KEY, harness TEXT NOT NULL, session_id TEXT NOT NULL,
            workspace TEXT NOT NULL, socket TEXT, UNIQUE(harness, session_id));
        CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
            sender TEXT NOT NULL REFERENCES peers(name), recipient TEXT NOT NULL REFERENCES peers(name),
            in_reply_to TEXT UNIQUE REFERENCES messages(id), body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
            submission TEXT NOT NULL CHECK(submission IN ('not_submitted', 'submission_unknown', 'submitted')),
            notification_started_at REAL, notification_finished_at REAL, notification_detail TEXT);
        INSERT INTO peers VALUES ('alice', 'codex', '{session}', '{ws}', '/old.sock');
        INSERT INTO peers VALUES ('bob', 'claude', '{other}', '{ws}', NULL);
        INSERT INTO messages (id, sender, recipient, body, created_at, ack_at, submission, notification_started_at,
            notification_finished_at, notification_detail)
            VALUES ('{request}', 'alice', 'bob', 'Old', 10.5, 11.0, 'submitted', 10.6, 10.7, 'claude_socket_bytes_written');
        PRAGMA user_version=1;", ws = temp.workspace(), other = new_id())).unwrap();
    let store = Store::open(&temp.db(), false).unwrap();
    let backup_path = fs::read_dir(temp.path().join("mail.sqlite3.backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        0o600,
        fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777
    );
    let backup = raw(&backup_path);
    assert_eq!(1, version(&backup));
    assert_eq!(5, columns(&backup, "peers").len());
    assert_eq!(
        "Old",
        backup
            .query_row("SELECT body FROM messages", [], |r| r.get::<_, String>(0))
            .unwrap()
    );
    let db = raw(&temp.db());
    assert_eq!(3, version(&db));
    assert_eq!(
        Some("delivery"),
        columns(&db, "peers").last().map(String::as_str)
    );
    let alice = store.peer("alice").unwrap();
    assert_eq!(
        (
            "codex",
            Some(session.as_str()),
            Some("/old.sock"),
            None,
            None
        ),
        (
            alice.harness.as_str(),
            alice.session_id.as_deref(),
            alice.socket.as_deref(),
            alice.url,
            alice.retired_at
        )
    );
    let old = store.get(&request, Some("bob")).unwrap();
    assert_eq!(
        (
            "Old",
            10.5,
            Some(11.0),
            Submission::Submitted,
            Some("claude_socket_bytes_written"),
            None
        ),
        (
            old.row.body.as_str(),
            old.row.created_at,
            old.row.ack_at,
            old.row.submission,
            old.row.notification_detail.as_deref(),
            old.row.wait_returned_at
        )
    );
    // Reopening a migrated database changes nothing.
    Store::open(&temp.db(), true).unwrap();
    assert_eq!(1, store.sent("alice", 20, None, true).unwrap().total);
}

#[test]
fn older_version_two_upgrades_on_open_and_preserves_messages() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let request = store
        .save("alice", "bob", "Question", None, None)
        .unwrap()
        .0;
    raw(&temp.db())
        .execute_batch(
            "DROP TABLE waits; DROP TABLE retired_peers;
        ALTER TABLE messages DROP COLUMN wait_returned_at",
        )
        .unwrap();
    schema_two(&temp.db());
    let reopened = Store::open(&temp.db(), false).unwrap();
    assert_eq!(
        None,
        reopened
            .get(&request.row.id, None)
            .unwrap()
            .row
            .wait_returned_at
    );
    reopened.retire("bob").unwrap();
    let db = raw(&temp.db());
    assert_eq!(3, version(&db));
    assert!(waits(&temp.db()).is_empty());
    // Existing native peers remain usable after the automatic upgrade.
    db.execute(
        "INSERT INTO peers (name, harness, session_id, workspace, socket, url) VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            "carol",
            "claude",
            new_id(),
            temp.workspace(),
            None::<String>,
            None::<String>
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO messages (id, sender, recipient, in_reply_to, body, created_at, submission)
        VALUES (?, ?, ?, ?, ?, ?, 'not_submitted')",
        rusqlite::params![
            new_id(),
            "alice",
            "carol",
            None::<String>,
            "Sent by 0.3",
            1.0
        ],
    )
    .unwrap();
    let names: Vec<_> = reopened
        .peers()
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.retired_at.is_some()))
        .collect();
    assert_eq!(
        vec![
            ("alice".into(), false),
            ("bob".into(), true),
            ("carol".into(), false)
        ],
        names
    );
    assert_eq!(1, reopened.inbox("carol", 20, None).unwrap().total);
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
fn every_connection_enforces_foreign_keys() {
    let temp = Temp::new();
    let store = store(&temp, &["alice"]);
    let db = store.connect().unwrap();
    assert_eq!(
        1,
        db.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap()
    );
    assert!(
        db.execute(
            "INSERT INTO messages (id, sender, recipient, body, created_at, submission)
        VALUES ('x', 'alice', 'nobody', 'b', 1, 'not_submitted')",
            []
        )
        .is_err()
    );
}

#[test]
fn busy_writer_retries_after_the_lock_is_released() {
    let temp = Temp::new();
    let store = store(&temp, &["alice"]);
    let holder = raw(&temp.db());
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let db = store.connect().unwrap();
        started_tx.send(()).unwrap();
        let result = db.execute_batch("BEGIN IMMEDIATE; ROLLBACK");
        done_tx.send(result).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    holder.execute_batch("COMMIT").unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    waiter.join().unwrap();
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
    // The budget counts requested sleep, so allow scheduler and I/O overhead.
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

#[test]
fn readonly_skip_check_never_creates_and_reports_only_codes() {
    let temp = Temp::new();
    let missing = temp.path().join("missing.sqlite3");
    assert_eq!(
        Err(Failure::coded("notification_state_unavailable")),
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
        Err(Failure::coded("notification_message_not_found")),
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
        Err(Failure::coded("notification_state_unavailable")),
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

#[test]
fn invalid_legacy_references_roll_back_the_entire_migration() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    store.save("alice", "bob", "Question", None, None).unwrap();
    schema_two(&temp.db());
    raw(&temp.db())
        .execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM peers WHERE name='bob'")
        .unwrap();
    let before = fs::read(temp.db()).unwrap();
    assert_eq!(
        "database_foreign_key_violation",
        code(Store::migrate(&temp.db()))
    );
    assert_eq!(before, fs::read(temp.db()).unwrap());
    assert_eq!(2, version(&raw(&temp.db())));
    assert!(!columns(&raw(&temp.db()), "peers").contains(&"delivery".into()));
}
