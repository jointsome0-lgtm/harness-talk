//! Generic mailboxes use the same durable message rules without native addresses.
#[path = "store_support.rs"]
mod support;
use harness_talk::{model::*, notify};
use support::*;

#[test]
fn pull_exchange_keeps_identity_reply_and_ack_rules_without_notifying() {
    let temp = Temp::new();
    let store = store(&temp, &[]);
    let alice = Peer::pull("alice", "future-harness");
    store.register(&alice).unwrap();
    store
        .register(&Peer::pull("bob", "future-harness"))
        .unwrap();
    assert_eq!(alice, store.register(&alice).unwrap());
    assert_eq!(
        "peer_already_has_a_different_address",
        code(store.register(&Peer::pull("alice", "different")))
    );
    let id = new_id();
    let request = store
        .save("alice", "bob", "Question", Some(&id), None)
        .unwrap()
        .0;
    assert_eq!(
        Some("pull_only"),
        request.row.notification_detail.as_deref()
    );
    let saved = store.notify_once(&id, &never, &skipped).unwrap();
    assert_eq!(Submission::NotSubmitted, saved.row.submission);
    assert_eq!(None, saved.row.notification_started_at);
    assert_eq!(None, saved.row.notification_finished_at);
    assert!(
        !store
            .save("alice", "bob", "Question", Some(&id), None)
            .unwrap()
            .1
    );
    assert_eq!(
        "message_id_conflict",
        code(store.save("alice", "bob", "Changed", Some(&id), None))
    );
    assert_eq!(None, store.get(&id, Some("bob")).unwrap().row.ack_at);
    store.ack(&id, "bob", &notify::dismiss).unwrap();
    assert_eq!(1, store.inbox("bob", 20, None).unwrap().total);
    assert_eq!(
        "only_recipient_can_ack",
        code(store.ack(&id, "alice", &notify::dismiss))
    );
    store.retire("bob").unwrap();
    assert_eq!(
        "peer_retired",
        code(store.save("alice", "bob", "New", None, None))
    );
    let answer = store
        .save("bob", "alice", "Answer", None, Some(&id))
        .unwrap()
        .0;
    assert_eq!(
        answer.row.id,
        store
            .save("bob", "alice", "Answer", None, Some(&id))
            .unwrap()
            .0
            .row
            .id
    );
    assert_eq!(
        "reply_conflict_existing_answer_preserved",
        code(store.save("bob", "alice", "Changed", None, Some(&id)))
    );
    assert_eq!(
        "reply_address_mismatch",
        code(store.save("alice", "bob", "Wrong", None, Some(&id)))
    );
    assert_eq!(
        answer.row.id,
        store.wait(&id, "alice", 0.0).unwrap().reply.unwrap().id
    );
    store
        .ack(&answer.row.id, "alice", &notify::dismiss)
        .unwrap();
    assert_eq!(0, store.inbox("alice", 20, None).unwrap().total);
}

#[test]
fn unknown_native_adapter_does_not_poison_mailbox_reads_or_ack() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    raw(&temp.db())
        .execute(
            "UPDATE peers SET harness='future-harness' WHERE name='bob'",
            [],
        )
        .unwrap();
    assert_eq!(2, store.peers().unwrap().len());
    let bob = store.peer("bob").unwrap();
    assert_eq!(
        "adapter_unavailable",
        notify::probe(&bob).unwrap_err().to_string()
    );
    let request = store
        .save("alice", "bob", "Question", None, None)
        .unwrap()
        .0;
    let sent = store
        .notify_once(
            &request.row.id,
            &|p, m| notify::notify(p, m, store.path()),
            &notify::dismiss,
        )
        .unwrap();
    assert_eq!(Submission::NotSubmitted, sent.row.submission);
    assert_eq!(
        Some("adapter_unavailable"),
        sent.row.notification_detail.as_deref()
    );
    assert_eq!(1, store.inbox("bob", 20, None).unwrap().total);
    assert_eq!(
        "Question",
        store.get(&request.row.id, Some("bob")).unwrap().row.body
    );
    assert!(
        store
            .ack(&request.row.id, "bob", &notify::dismiss)
            .unwrap()
            .row
            .ack_at
            .is_some()
    );
    // Even a failed native attempt is never replayed.
    store
        .notify_once(&request.row.id, &never, &skipped)
        .unwrap();
}
