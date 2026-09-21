//! Saving, idempotency, reading, acknowledgment, waiting and pages.
#[path = "store_support.rs"]
mod support;

use harness_talk::model::*;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use std::{
    thread,
    time::{Duration, Instant},
};
use support::*;

#[test]
fn save_checks_run_in_order() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    assert_eq!(
        "message_must_be_1_to_32000_bytes",
        code(store.save("nobody", "nobody", " \n", None, None))
    );
    assert_eq!(
        "message_must_be_1_to_32000_bytes",
        code(store.save("alice", "bob", &"я".repeat(16001), None, None))
    );
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
        code(store.save("alice", "bob", "Hi", Some("bad"), Some("unknown")))
    );
    assert_eq!(
        "unknown_request",
        code(store.save("alice", "bob", "Hi", None, Some("unknown")))
    );
    assert_eq!(0, store.sent("alice", 20, None, true).unwrap().total);
    let (message, created) = store
        .save("alice", "bob", &"я".repeat(16000), None, None)
        .unwrap();
    assert!(created);
    assert_eq!(
        (MessageState::Saved, Submission::NotSubmitted, None),
        (
            message.state,
            message.row.submission,
            message.row.in_reply_to
        )
    );
}

#[test]
fn retries_are_idempotent_and_conflicts_preserve_the_first_write() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob", "eve"]);
    let id = new_id();
    let (question, created) = store
        .save("alice", "bob", "Question?", Some(&id.to_uppercase()), None)
        .unwrap();
    assert_eq!((id.as_str(), true), (question.row.id.as_str(), created));
    let (again, created) = store
        .save("alice", "bob", "Question?", Some(&id), None)
        .unwrap();
    assert_eq!((question.row.seq, false), (again.row.seq, created));
    assert_eq!(
        "message_id_conflict",
        code(store.save("alice", "bob", "Changed", Some(&id), None))
    );
    assert_eq!(
        "message_id_conflict",
        code(store.save("alice", "eve", "Question?", Some(&id), None))
    );
    assert_eq!(
        "reply_address_mismatch",
        code(store.save("eve", "alice", "Forged", None, Some(&id)))
    );
    let (answer, created) = store
        .save("bob", "alice", "Answer", None, Some(&id))
        .unwrap();
    assert!(created);
    // Reply idempotency comes before ID idempotency: any ID returns the saved answer.
    let (again, created) = store
        .save("bob", "alice", "Answer", Some(&new_id()), Some(&id))
        .unwrap();
    assert_eq!(
        (answer.row.id.as_str(), false),
        (again.row.id.as_str(), created)
    );
    assert_eq!(
        "reply_conflict_existing_answer_preserved",
        code(store.save("bob", "alice", "Changed", None, Some(&id)))
    );
    assert_eq!(
        "reply_requires_a_request",
        code(store.save("alice", "bob", "Re: answer", None, Some(&answer.row.id)))
    );
    // The answer's own ID sent as a request is a conflict, not a new message.
    assert_eq!(
        "message_id_conflict",
        code(store.save("bob", "alice", "Answer", Some(&answer.row.id), None))
    );
    let request = store.get(&id, None).unwrap();
    assert_eq!(
        (MessageState::ReplyReceived, Some("Answer")),
        (
            request.state,
            request.reply.as_ref().map(|r| r.body.as_str())
        )
    );
}

#[test]
fn retirement_refuses_new_requests_but_not_answers_or_retries() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let before = store.save("alice", "bob", "Before", None, None).unwrap().0;
    let incoming = store
        .save("bob", "alice", "From bob", None, None)
        .unwrap()
        .0;
    store.retire("bob").unwrap();
    assert_eq!(
        "peer_retired",
        code(store.save("alice", "bob", "After", None, None))
    );
    assert_eq!(
        "peer_retired",
        code(store.save("bob", "alice", "After", None, None))
    );
    assert_eq!(
        (1, 1),
        (
            store.sent("alice", 20, None, true).unwrap().total,
            store.sent("bob", 20, None, true).unwrap().total
        )
    );
    assert!(
        !store
            .save("alice", "bob", "Before", Some(&before.row.id), None)
            .unwrap()
            .1
    );
    assert!(
        store
            .save("alice", "bob", "Answer", None, Some(&incoming.row.id))
            .unwrap()
            .1
    );
    assert!(
        store
            .save("bob", "alice", "Late answer", None, Some(&before.row.id))
            .unwrap()
            .1
    );
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

#[test]
fn reads_and_acknowledgments_are_limited_to_the_addressed_peers() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob", "eve"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    assert_eq!(
        "message_not_addressed_to_peer",
        code(store.get(&question.row.id, Some("eve")))
    );
    assert_eq!("unknown_message", code(store.get(&new_id(), None)));
    assert_eq!(
        "only_recipient_can_ack",
        code(store.ack(&question.row.id, "alice", &skipped))
    );
    assert_eq!(
        "wait_requires_own_request",
        code(store.wait(&question.row.id, "bob", 0.0))
    );
    assert_eq!(
        "wait_seconds_must_be_between_0_and_45",
        code(store.wait(&new_id(), "alice", f64::NAN))
    );
    assert_eq!(
        "wait_seconds_must_be_between_0_and_45",
        code(store.wait(&question.row.id, "alice", 45.5))
    );
    // A lost notice and an ack do not hide an unanswered question.
    let first = store.ack(&question.row.id, "bob", &skipped).unwrap();
    let again = store.ack(&question.row.id, "bob", &skipped).unwrap();
    assert_eq!(first.row.ack_at, again.row.ack_at);
    assert_eq!(
        Some(CleanupStatus::Skipped),
        again.notification_cleanup.map(|c| c.status)
    );
    assert_eq!(
        vec![question.row.id.clone()],
        ids(&store.inbox("bob", 20, None).unwrap())
    );
    let reply = store
        .save("bob", "alice", "Answer", None, Some(&question.row.id))
        .unwrap()
        .0;
    assert!(store.inbox("bob", 20, None).unwrap().messages.is_empty());
    assert_eq!(
        vec![reply.row.id.clone()],
        ids(&store.inbox("alice", 20, None).unwrap())
    );
    store.ack(&reply.row.id, "alice", &skipped).unwrap();
    assert!(store.inbox("alice", 20, None).unwrap().messages.is_empty());
}

#[test]
fn message_json_has_the_native_row_shape() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let value = serde_json::to_value(&question).unwrap();
    let keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        vec![
            "seq",
            "id",
            "sender",
            "recipient",
            "in_reply_to",
            "body",
            "created_at",
            "ack_at",
            "submission",
            "notification_started_at",
            "notification_finished_at",
            "notification_detail",
            "wait_returned_at",
            "reply",
            "state"
        ],
        keys
    );
    assert_eq!(
        (&json!(null), &json!("saved"), &json!("not_submitted")),
        (&value["reply"], &value["state"], &value["submission"])
    );
    store
        .save("bob", "alice", "Answer", None, Some(&question.row.id))
        .unwrap();
    let page = store.inbox("alice", 20, None).unwrap();
    assert_eq!(
        Some("Read and ack messages explicitly. Unanswered questions remain until replied to."),
        page.next_action.as_deref()
    );
    assert_eq!(json!(question.row.id), page.messages[0]["in_reply_to"]);
    assert!(
        store
            .sent("alice", 20, None, true)
            .unwrap()
            .next_action
            .is_none()
    );
}

fn ids(page: &Page) -> Vec<String> {
    page.messages
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}
fn shape(pages: &[Page]) -> Vec<(usize, i64, i64)> {
    pages
        .iter()
        .map(|p| (p.messages.len(), p.total, p.omitted))
        .collect()
}
fn last_seq(page: &Page) -> i64 {
    page.messages.last().unwrap()["seq"].as_i64().unwrap()
}

#[test]
fn sent_pages_cover_a_long_history_newest_first_without_gaps() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let created = bulk_requests(&temp.db(), "alice", "bob", 520);
    let first = store.sent("alice", 500, None, false).unwrap();
    let second = store
        .sent("alice", 500, Some(last_seq(&first)), false)
        .unwrap();
    assert_eq!(
        vec![(500, 520, 20), (20, 520, 0)],
        shape(&[first.clone(), second.clone()])
    );
    let listed: Vec<_> = [ids(&first), ids(&second)].concat();
    assert_eq!(created.iter().rev().cloned().collect::<Vec<_>>(), listed);
    let past_end = store.sent("alice", 500, Some(1), false).unwrap();
    assert_eq!(
        (0, 520, 0),
        (past_end.messages.len(), past_end.total, past_end.omitted)
    );
    assert_eq!(
        520,
        store
            .sent("alice", 1, Some(i64::MAX), false)
            .unwrap()
            .omitted
            + 1
    );
}

#[test]
fn inbox_pages_keep_open_questions_oldest_first() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let acknowledged = store
        .save("alice", "bob", "Acknowledged but unanswered", None, None)
        .unwrap()
        .0;
    store.ack(&acknowledged.row.id, "bob", &skipped).unwrap();
    let answered = store
        .save("alice", "bob", "Answered", None, None)
        .unwrap()
        .0;
    store
        .save("bob", "alice", "Done", None, Some(&answered.row.id))
        .unwrap();
    let open = [
        vec![acknowledged.row.id.clone()],
        bulk_requests(&temp.db(), "alice", "bob", 4),
    ]
    .concat();
    let mut pages = vec![store.inbox("bob", 2, None).unwrap()];
    while pages.last().unwrap().omitted > 0 {
        let cursor = last_seq(pages.last().unwrap());
        pages.push(store.inbox("bob", 2, Some(cursor)).unwrap());
    }
    assert_eq!(vec![(2, 5, 3), (2, 5, 1), (1, 5, 0)], shape(&pages));
    assert_eq!(open, pages.iter().flat_map(ids).collect::<Vec<_>>());
    assert_eq!("Acknowledged but unanswered", pages[0].messages[0]["body"]);
}

#[test]
fn invalid_limits_cursors_and_actors_are_errors() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    assert_eq!(
        "limit_must_be_between_1_and_500",
        code(store.sent("alice", 0, None, false))
    );
    assert_eq!(
        "limit_must_be_between_1_and_500",
        code(store.inbox("alice", 501, None))
    );
    assert_eq!(
        "seq_cursor_must_be_a_positive_integer",
        code(store.sent("alice", 20, Some(0), false))
    );
    assert_eq!(
        "seq_cursor_must_be_a_positive_integer",
        code(store.inbox("alice", 20, Some(-1)))
    );
    assert_eq!("unknown_peer", code(store.inbox("carol", 20, None)));
}

#[test]
fn sent_summarizes_texts_of_requests_and_answers_unless_bodies_requested() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let body = "\n   \n  Привет — первая строка  \nвторая строка";
    let question = store.save("alice", "bob", body, None, None).unwrap().0;
    store
        .save(
            "bob",
            "alice",
            "Ответ\nподробности",
            None,
            Some(&question.row.id),
        )
        .unwrap();
    let long = store
        .save("alice", "bob", &"я".repeat(200), None, None)
        .unwrap()
        .0;
    let page = store.sent("alice", 20, None, false).unwrap();
    let find = |id: &str| {
        page.messages
            .iter()
            .find(|m| m["id"] == id)
            .unwrap()
            .clone()
    };
    let summary = find(&question.row.id);
    assert!(summary.get("body").is_none() && summary["reply"].get("body").is_none());
    assert_eq!(
        (json!(body.len()), json!("Привет — первая строка")),
        (
            summary["body_bytes"].clone(),
            summary["body_preview"].clone()
        )
    );
    assert_eq!(json!("Ответ"), summary["reply"]["body_preview"]);
    assert_eq!(
        Value::from("я".repeat(120)),
        find(&long.row.id)["body_preview"]
    );
    let keys: Vec<_> = summary
        .as_object()
        .unwrap()
        .keys()
        .rev()
        .take(4)
        .map(String::as_str)
        .collect();
    assert_eq!(vec!["body_preview", "body_bytes", "state", "reply"], keys);
    assert_eq!(
        body,
        store.get(&question.row.id, Some("alice")).unwrap().row.body
    );
    let full = store.sent("alice", 20, None, true).unwrap();
    assert_eq!(Value::from("я".repeat(200)), full.messages[0]["body"]);
    assert_eq!(
        json!("Ответ\nподробности"),
        full.messages[1]["reply"]["body"]
    );
}

#[test]
fn wait_returns_the_answer_records_the_receipt_and_forgets_its_registration() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    let ended = store.wait(&question.row.id, "alice", 0.2).unwrap();
    assert_eq!(Some("timeout"), ended.wait_ended.as_deref());
    assert_eq!(
        json!("timeout"),
        serde_json::to_value(&ended).unwrap()["wait_ended"]
    );
    assert!(waits(&temp.db()).is_empty());
    let answer = thread::scope(|scope| {
        let waiting = scope.spawn(|| store.wait(&question.row.id, "alice", 10.0).unwrap());
        assert!(until(Duration::from_secs(5), || !waits(&temp.db()).is_empty()));
        let (id, actor, _) = waits(&temp.db()).remove(0);
        assert_eq!(
            (question.row.id.as_str(), "alice"),
            (id.as_str(), actor.as_str())
        );
        let answer = store
            .save("bob", "alice", "Late", None, Some(&question.row.id))
            .unwrap()
            .0;
        let started = Instant::now();
        let received = waiting.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            (None, Some("Late")),
            (
                received.wait_ended.as_deref(),
                received.reply.as_ref().map(|r| r.body.as_str())
            )
        );
        assert!(received.reply.unwrap().wait_returned_at.is_some());
        answer
    });
    assert!(waits(&temp.db()).is_empty());
    let saved = store.get(&answer.row.id, None).unwrap();
    assert_eq!(
        (true, None),
        (saved.row.wait_returned_at.is_some(), saved.row.ack_at)
    );
    // The receipt keeps its first time and is not an acknowledgment.
    let first = saved.row.wait_returned_at;
    assert_eq!(
        first,
        store
            .wait(&question.row.id, "alice", 0.0)
            .unwrap()
            .reply
            .unwrap()
            .wait_returned_at
    );
    assert_eq!(
        vec![answer.row.id.clone()],
        ids(&store.inbox("alice", 20, None).unwrap())
    );
}

#[test]
fn zero_second_wait_checks_once_without_registering() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let question = store
        .save("alice", "bob", "Question?", None, None)
        .unwrap()
        .0;
    // A stale row older than a minute is pruned by the next registration only.
    raw(&temp.db())
        .execute(
            "INSERT INTO waits VALUES ('stale', ?, 'alice', 1.0)",
            [&question.row.id],
        )
        .unwrap();
    assert_eq!(
        Some("timeout"),
        store
            .wait(&question.row.id, "alice", 0.0)
            .unwrap()
            .wait_ended
            .as_deref()
    );
    assert_eq!(1, waits(&temp.db()).len());
    store.wait(&question.row.id, "alice", 0.05).unwrap();
    assert!(waits(&temp.db()).is_empty());
    let answer = store
        .save("bob", "alice", "Answer", None, Some(&question.row.id))
        .unwrap()
        .0;
    assert_eq!(
        None,
        store
            .get(&answer.row.id, None)
            .unwrap()
            .row
            .wait_returned_at
    );
    let reverse = store
        .save("bob", "alice", "Reverse?", None, None)
        .unwrap()
        .0;
    store
        .save("alice", "bob", "Yes", None, Some(&reverse.row.id))
        .unwrap();
    assert_eq!(
        MessageState::ReplyReceived,
        store.wait(&reverse.row.id, "bob", 0.0).unwrap().state
    );
}
