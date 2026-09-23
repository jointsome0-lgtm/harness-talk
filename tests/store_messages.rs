//! Concurrent replies and validation while a writer holds the database.
#[path = "store_support.rs"]
mod support;

use std::sync::{Arc, Barrier};
use std::thread;
use support::*;

#[test]
fn invalid_requests_do_not_wait_for_a_writer_and_keep_peer_error_precedence() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let writer = store.connect().unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        "unknown_peer",
        code(store.save("alice", "nobody", "Hi", Some("bad"), None))
    );
    assert_eq!(
        "sender_and_recipient_must_differ",
        code(store.save("alice", "alice", "Hi", Some("bad"), None))
    );
    assert_eq!(
        "badly formed hexadecimal UUID string",
        code(store.save("alice", "bob", "Hi", Some("bad"), None))
    );
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn concurrent_replies_save_one_answer() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (path, id, barrier) = (temp.db(), question.row.id.clone(), barrier.clone());
            thread::spawn(move || {
                let store = harness_talk::store::Store::open(&path, false).unwrap();
                barrier.wait();
                let (answer, created) =
                    store.save("bob", "alice", "Same", None, Some(&id)).unwrap();
                (answer.row.id, created)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(1, results.iter().filter(|(_, created)| *created).count());
    assert!(results.iter().all(|(id, _)| *id == results[0].0));
}
