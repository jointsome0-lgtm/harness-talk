//! One notification attempt per message: claim before the call, suppression by
//! acknowledgment, wait receipt or retirement, and acknowledgment cleanup.
#[path = "store_support.rs"]
mod support;

use harness_talk::{
    model::*,
    store::{self, Store, WAIT_GRACE},
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::{
    thread,
    time::{Duration, Instant},
};
use support::*;

fn submitted(_: &Peer, _: &Message) -> Outcome {
    Outcome::submitted("claude_socket_bytes_written")
}

#[test]
fn the_claim_is_durable_before_the_call_and_the_attempt_is_never_repeated() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let calls = Cell::new(0);
    let notify = |peer: &Peer, message: &Message| {
        calls.set(calls.get() + 1);
        let saved = Store::open(&temp.db(), false)
            .unwrap()
            .get(&message.row.id, None)
            .unwrap();
        assert_eq!(
            (Submission::SubmissionUnknown, true, None),
            (
                saved.row.submission,
                saved.row.notification_started_at.is_some(),
                saved.row.notification_finished_at
            )
        );
        assert_eq!(
            ("bob", Submission::SubmissionUnknown),
            (peer.name.as_str(), message.row.submission)
        );
        Outcome::not_submitted("recipient_unavailable")
    };
    let result = store
        .notify_once(&question.row.id, &notify, &never_cleanup)
        .unwrap();
    assert_eq!(
        (
            Submission::NotSubmitted,
            Some("recipient_unavailable"),
            true
        ),
        (
            result.row.submission,
            result.row.notification_detail.as_deref(),
            result.row.notification_finished_at.is_some()
        )
    );
    assert!(result.notification_cleanup.is_none());
    let again = Store::open(&temp.db(), false)
        .unwrap()
        .notify_once(&question.row.id, &notify, &never_cleanup)
        .unwrap();
    assert_eq!(1, calls.get());
    assert_eq!(
        result.row.notification_finished_at,
        again.row.notification_finished_at
    );
    assert_eq!(
        "unknown_message",
        code(store.notify_once(&new_id(), &never, &never_cleanup))
    );
}

fn never_cleanup(_: &Peer, _: &Message) -> Cleanup {
    panic!("cleanup must not run without an acknowledgment")
}

#[test]
fn an_interrupted_claim_stays_unknown_and_is_not_replayed() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    // A process that died inside the client call leaves only its claim.
    raw(&temp.db()).execute("UPDATE messages SET submission='submission_unknown', notification_started_at=1 WHERE id=?",
                            [&question.row.id]).unwrap();
    let result = store
        .notify_once(&question.row.id, &never, &never_cleanup)
        .unwrap();
    assert_eq!(
        (Submission::SubmissionUnknown, None),
        (result.row.submission, result.row.notification_finished_at)
    );
    assert_eq!(
        vec![question.row.id.clone()],
        store
            .sent("alice", 20, None, false)
            .unwrap()
            .messages
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_acknowledgment_before_the_claim_prevents_it() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    store.ack(&question.row.id, "bob", &skipped).unwrap();
    let result = store
        .notify_once(&question.row.id, &never, &never_cleanup)
        .unwrap();
    assert_eq!(
        (Submission::NotSubmitted, None, None),
        (
            result.row.submission,
            result.row.notification_started_at,
            result.row.notification_detail
        )
    );
    assert!(result.notification_cleanup.is_none());
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
fn a_wait_that_already_returned_the_answer_suppresses_its_notice() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save(
            "builder",
            "reviewer",
            "Which case needs another test?",
            None,
            None,
        )
        .unwrap()
        .0;
    let answer = store
        .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
        .unwrap()
        .0;
    assert!(
        store
            .wait(&request.row.id, "builder", 0.0)
            .unwrap()
            .reply
            .is_some()
    );
    assert_eq!(
        Some(SkipReason::ReturnedByRecipientWait),
        store.skip_reason(&answer).unwrap()
    );
    let result = Store::open(&temp.db(), false)
        .unwrap()
        .notify_once(&answer.row.id, &never, &never_cleanup)
        .unwrap();
    assert_eq!(
        (
            Submission::NotSubmitted,
            Some("returned_by_recipient_wait"),
            None
        ),
        (
            result.row.submission,
            result.row.notification_detail.as_deref(),
            result.row.ack_at
        )
    );
    assert!(result.row.notification_finished_at.is_some());
    assert_eq!(1, store.inbox("builder", 20, None).unwrap().total);
    store.ack(&answer.row.id, "builder", &skipped).unwrap();
    assert_eq!(
        Some(SkipReason::AcknowledgedBeforeNotification),
        store.skip_reason(&answer).unwrap()
    );
}

#[test]
fn a_reply_saved_during_a_running_wait_gives_the_poll_time_to_return_it() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save("builder", "reviewer", "Question", None, None)
        .unwrap()
        .0;
    thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            Store::open(&temp.db(), false)
                .unwrap()
                .wait(&request.row.id, "builder", 10.0)
                .unwrap()
        });
        assert!(until(Duration::from_secs(5), || !waits(&temp.db()).is_empty()));
        let answer = store
            .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
            .unwrap()
            .0;
        let started = Instant::now();
        let result = store
            .notify_once(&answer.row.id, &never, &never_cleanup)
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs_f64(WAIT_GRACE));
        assert_eq!(
            (Submission::NotSubmitted, Some("returned_by_recipient_wait")),
            (
                result.row.submission,
                result.row.notification_detail.as_deref()
            )
        );
        assert_eq!(answer.row.id, waiting.join().unwrap().reply.unwrap().id);
    });
    assert!(waits(&temp.db()).is_empty());
}

#[test]
fn a_stale_wait_registration_only_delays_the_notice_by_the_grace_bound() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save("builder", "reviewer", "Question", None, None)
        .unwrap()
        .0;
    // A killed waiter leaves its registration behind.
    raw(&temp.db())
        .execute(
            "INSERT INTO waits VALUES ('killed', ?, 'builder', ?)",
            rusqlite::params![request.row.id, harness_talk::os::now() + 45.0],
        )
        .unwrap();
    let answer = store
        .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
        .unwrap()
        .0;
    let started = Instant::now();
    let result = store
        .notify_once(&answer.row.id, &submitted, &never_cleanup)
        .unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    assert_eq!(
        (Submission::Submitted, None),
        (result.row.submission, result.row.wait_returned_at)
    );
    assert!(
        (WAIT_GRACE * 0.9..WAIT_GRACE + 1.0).contains(&elapsed),
        "{elapsed}"
    );
    // A registration by someone else, or on another request, costs nothing.
    let other = store
        .save("builder", "reviewer", "Other", None, None)
        .unwrap()
        .0;
    raw(&temp.db())
        .execute(
            "INSERT INTO waits VALUES ('other', ?, 'reviewer', ?)",
            rusqlite::params![other.row.id, harness_talk::os::now() + 45.0],
        )
        .unwrap();
    let reply = store
        .save(
            "reviewer",
            "builder",
            "Other answer",
            None,
            Some(&other.row.id),
        )
        .unwrap()
        .0;
    let started = Instant::now();
    store
        .notify_once(&reply.row.id, &submitted, &never_cleanup)
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn a_notice_to_a_retired_recipient_is_skipped_and_not_replayed_after_restore() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("bob", "alice", "Question", None, None)
        .unwrap()
        .0;
    store.retire("bob").unwrap();
    let answer = store
        .save("alice", "bob", "Answer", None, Some(&question.row.id))
        .unwrap()
        .0;
    let result = store
        .notify_once(&answer.row.id, &never, &never_cleanup)
        .unwrap();
    assert_eq!(
        (Submission::NotSubmitted, Some("recipient_retired")),
        (
            result.row.submission,
            result.row.notification_detail.as_deref()
        )
    );
    store.restore("bob").unwrap();
    assert_eq!(
        Some("recipient_retired"),
        store
            .notify_once(&answer.row.id, &never, &never_cleanup)
            .unwrap()
            .row
            .notification_detail
            .as_deref()
    );
    let welcome = store
        .save("alice", "bob", "Welcome back", None, None)
        .unwrap()
        .0;
    assert_eq!(
        Submission::Submitted,
        store
            .notify_once(&welcome.row.id, &submitted, &never_cleanup)
            .unwrap()
            .row
            .submission
    );
    // Retirement during the client's preflight is seen by the transport's final check.
    let late = store.save("alice", "bob", "Another", None, None).unwrap().0;
    let notify = |peer: &Peer, message: &Message| {
        Store::open(&temp.db(), false)
            .unwrap()
            .retire("bob")
            .unwrap();
        match store::skip_reason_readonly(&temp.db(), &message.row.id, &peer.name) {
            Ok(Some(reason)) => Outcome::not_submitted(reason.as_str()),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(
        Some("recipient_retired"),
        store
            .notify_once(&late.row.id, &notify, &never_cleanup)
            .unwrap()
            .row
            .notification_detail
            .as_deref()
    );
}

#[test]
fn a_wait_completing_during_preflight_is_seen_by_the_final_check() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save("builder", "reviewer", "Question", None, None)
        .unwrap()
        .0;
    let answer = store
        .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
        .unwrap()
        .0;
    let notify = |peer: &Peer, message: &Message| {
        assert_eq!(
            Ok(None),
            store::skip_reason_readonly(&temp.db(), &message.row.id, &peer.name)
        );
        Store::open(&temp.db(), false)
            .unwrap()
            .wait(&request.row.id, "builder", 5.0)
            .unwrap();
        match store::skip_reason_readonly(&temp.db(), &message.row.id, &peer.name) {
            Ok(Some(reason)) => Outcome::not_submitted(reason.as_str()),
            other => panic!("{other:?}"),
        }
    };
    let result = store
        .notify_once(&answer.row.id, &notify, &never_cleanup)
        .unwrap();
    assert_eq!(
        (
            Submission::NotSubmitted,
            Some("returned_by_recipient_wait"),
            None
        ),
        (
            result.row.submission,
            result.row.notification_detail.as_deref(),
            result.row.ack_at
        )
    );
}

#[test]
fn a_passive_read_does_not_suppress_the_notice() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save("builder", "reviewer", "Question", None, None)
        .unwrap()
        .0;
    let answer = store
        .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
        .unwrap()
        .0;
    store.get(&answer.row.id, Some("builder")).unwrap();
    store.inbox("builder", 20, None).unwrap();
    store.sent("reviewer", 20, None, false).unwrap();
    assert_eq!(None, store.skip_reason(&answer).unwrap());
    assert_eq!(
        Submission::Submitted,
        store
            .notify_once(&answer.row.id, &submitted, &never_cleanup)
            .unwrap()
            .row
            .submission
    );
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
