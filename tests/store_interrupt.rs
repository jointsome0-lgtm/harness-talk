//! SIGINT during a wait. A separate test binary, because the interrupt flag is process-wide.
#[path = "store_support.rs"]
mod support;

use harness_talk::{error::Error, model::*, os};
use std::time::{Duration, Instant};
use support::*;

#[test]
fn an_interrupted_wait_forgets_its_registration_and_records_nothing() {
    let temp = Temp::new();
    let store = store(&temp, &["builder", "reviewer"]);
    let request = store
        .save("builder", "reviewer", "Question", None, None)
        .unwrap()
        .0;
    os::install_interrupt_handler().unwrap();
    assert_eq!(0, unsafe { libc::raise(libc::SIGINT) });
    assert!(os::interrupted());
    let started = Instant::now();
    assert!(matches!(
        store.wait(&request.row.id, "builder", 5.0),
        Err(Error::Interrupted)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(waits(&temp.db()).is_empty());
    // A single check still completes, and one that returns the answer records the receipt.
    assert_eq!(
        Some("timeout"),
        store
            .wait(&request.row.id, "builder", 0.0)
            .unwrap()
            .wait_ended
            .as_deref()
    );
    let answer = store
        .save("reviewer", "builder", "Answer", None, Some(&request.row.id))
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
    assert_eq!(
        answer.row.id,
        store
            .wait(&request.row.id, "builder", 5.0)
            .unwrap()
            .reply
            .unwrap()
            .id
    );
    assert_eq!(
        Some(SkipReason::ReturnedByRecipientWait),
        store.skip_reason(&answer).unwrap()
    );
    assert!(waits(&temp.db()).is_empty());
}
