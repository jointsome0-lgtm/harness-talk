//! One notification attempt per message: claim before the call, suppression by
//! acknowledgment, wait receipt or retirement, and acknowledgment cleanup.
#[path = "store_support.rs"]
mod support;

use harness_talk::{
    model::*,
    store::{self, Store},
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::thread;
use support::*;

fn never_cleanup(_: &Peer, _: &Message) -> Cleanup {
    panic!("cleanup must not run without an acknowledgment")
}

#[test]
fn an_acknowledgment_during_submission_is_cleaned_up_when_the_receipt_arrives() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let queue = RefCell::new(HashSet::from(["unrelated-user-input".to_owned()]));
    let dismiss = |peer: &Peer, message: &Message| {
        assert_eq!("bob", peer.name);
        assert!(message.row.ack_at.is_some());
        let Some(queue_id) = message
            .row
            .notification_detail
            .as_deref()
            .and_then(|d| d.strip_prefix("codex_cli_queued:"))
        else {
            return Cleanup::new(CleanupStatus::Skipped);
        };
        let removed = queue.borrow_mut().remove(queue_id);
        Cleanup::new(if removed {
            CleanupStatus::Removed
        } else {
            CleanupStatus::Absent
        })
        .queue_id(queue_id)
    };
    let inner = RefCell::new(None);
    let notify = |_: &Peer, message: &Message| {
        // The recipient reads and acknowledges while the client call is still running.
        let reader = Store::open(&temp.db(), false).unwrap();
        *inner.borrow_mut() = Some(reader.ack(&message.row.id, "bob", &dismiss).unwrap());
        assert_eq!(
            Ok(Some(SkipReason::AcknowledgedBeforeNotification)),
            store::skip_reason_readonly(&temp.db(), &message.row.id, "bob")
        );
        queue.borrow_mut().insert("q1".into());
        Outcome::submitted("codex_cli_queued:q1")
    };
    let result = store
        .notify_once(&question.row.id, &notify, &dismiss)
        .unwrap();
    assert_eq!(
        Some(CleanupStatus::Skipped),
        inner
            .borrow()
            .as_ref()
            .and_then(|m| m.notification_cleanup.as_ref())
            .map(|c| c.status)
    );
    let cleanup = result.notification_cleanup.clone().unwrap();
    assert_eq!(
        (CleanupStatus::Removed, Some("q1")),
        (cleanup.status, cleanup.queue_id.as_deref())
    );
    assert_eq!(
        HashSet::from(["unrelated-user-input".to_owned()]),
        *queue.borrow()
    );
    assert_eq!(Submission::Submitted, result.row.submission);
    // A repeated ack retries the same cleanup without changing the acknowledgment.
    let again = store.ack(&question.row.id, "bob", &dismiss).unwrap();
    assert_eq!(
        (result.row.ack_at, CleanupStatus::Absent),
        (again.row.ack_at, again.notification_cleanup.unwrap().status)
    );
    let json = serde_json::to_value(&again.row).unwrap();
    assert_eq!("codex_cli_queued:q1", json["notification_detail"]);
}

#[test]
fn concurrent_notifiers_make_one_attempt() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let notify = |_: &Peer, _: &Message| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Outcome::unknown("codex_queue_receipt_mismatch")
    };
    thread::scope(|scope| {
        for _ in 0..6 {
            scope.spawn(|| {
                Store::open(&temp.db(), false)
                    .unwrap()
                    .notify_once(&question.row.id, &notify, &never_cleanup)
                    .unwrap()
            });
        }
    });
    assert_eq!(1, calls.into_inner());
    let saved = store.get(&question.row.id, None).unwrap();
    assert_eq!(
        (
            Submission::SubmissionUnknown,
            Some("codex_queue_receipt_mismatch")
        ),
        (
            saved.row.submission,
            saved.row.notification_detail.as_deref()
        )
    );
}
