//! Shared fixtures for the store tests, included with `#[path]`. Standalone it has no tests.
#![allow(dead_code)]
use harness_talk::{error::Error, model::*, store::Store};
use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

/// An explicit temporary directory, removed on drop.
pub struct Temp(pub PathBuf);
impl Temp {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("htalk-store-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Temp(fs::canonicalize(path).unwrap())
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn db(&self) -> PathBuf {
        self.0.join("mail.sqlite3")
    }
    pub fn workspace(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A new store with one Claude peer per name.
pub fn store(temp: &Temp, names: &[&str]) -> Store {
    let store = Store::open(&temp.db(), true).unwrap();
    for name in names {
        let peer = harness_talk::notify::native_peer(
            name,
            Harness::Claude,
            &new_id(),
            temp.workspace(),
            None,
            None,
        )
        .unwrap();
        store.register(&peer).unwrap();
    }
    store
}

pub fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

pub fn code<T: Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(Error::Code(c)) => c,
        other => panic!("expected a coded error, got {other:?}"),
    }
}

pub fn skipped(_: &Peer, _: &Message) -> Cleanup {
    Cleanup::new(CleanupStatus::Skipped)
}
pub fn waits(path: &Path) -> Vec<(String, String, f64)> {
    let db = raw(path);
    let mut stmt = db
        .prepare("SELECT message_id, actor, until FROM waits")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Reconstruct the historical six-column peer table for migration fixtures.
pub fn schema_two(path: &Path) {
    raw(path)
        .execute_batch(
            "PRAGMA foreign_keys=OFF; CREATE TABLE peers_v2 (
        name TEXT PRIMARY KEY, harness TEXT NOT NULL, session_id TEXT NOT NULL,
        workspace TEXT NOT NULL, socket TEXT, url TEXT, UNIQUE(harness, session_id));
        INSERT INTO peers_v2 SELECT name, harness, session_id, workspace, socket, url FROM peers;
        DROP TABLE peers; ALTER TABLE peers_v2 RENAME TO peers;
        PRAGMA user_version=2;",
        )
        .unwrap();
}
