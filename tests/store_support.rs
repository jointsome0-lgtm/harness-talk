//! Shared fixtures for the store tests, included with `#[path]`. Standalone it has no tests.
#![allow(dead_code)]
use harness_talk::{error::Error, model::*, store::Store};
use std::fmt::Debug;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

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
        fn open(path: &Path) {
            if let Ok(meta) = fs::symlink_metadata(path)
                && meta.is_dir()
            {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                if let Ok(entries) = fs::read_dir(path) {
                    entries.flatten().for_each(|e| open(&e.path()));
                }
            }
        }
        open(&self.0);
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
        store
            .add_peer(
                name,
                Harness::Claude,
                &new_id(),
                temp.workspace(),
                None,
                None,
            )
            .unwrap();
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
pub fn never(_: &Peer, _: &Message) -> Outcome {
    panic!("notification must not be attempted")
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

pub fn until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Direct inserts keep a history above the maximum page size fast to build.
pub fn bulk_requests(path: &Path, sender: &str, recipient: &str, count: usize) -> Vec<String> {
    let mut db = raw(path);
    let tx = db.transaction().unwrap();
    let ids: Vec<String> = (0..count).map(|_| new_id()).collect();
    for (n, id) in ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO messages (id, sender, recipient, body, created_at, submission)
            VALUES (?, ?, ?, ?, ?, 'not_submitted')",
            rusqlite::params![id, sender, recipient, format!("Question {n}"), 1.0],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    ids
}
