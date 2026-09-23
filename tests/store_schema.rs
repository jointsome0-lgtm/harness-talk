//! Opening, creation, migration and per-connection settings of the shared database.
#[path = "store_support.rs"]
mod support;

use harness_talk::{
    error::{Error, Failure},
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
