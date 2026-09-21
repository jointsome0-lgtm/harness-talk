//! SIGINT during a wait. A separate test binary, because the interrupt flag is process-wide.
#[path = "store_support.rs"]
mod support;

use harness_talk::{error::Error, os};
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
    assert!(matches!(
        store.wait(&request.row.id, "builder", 5.0),
        Err(Error::Interrupted)
    ));
    assert!(waits(&temp.db()).is_empty());
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
    // An answer appearing alongside SIGINT must remain eligible for its notice:
    // the CLI returns interrupted, so it did not deliver the answer's body.
    for seconds in [5.0, 0.0] {
        assert!(matches!(
            store.wait(&request.row.id, "builder", seconds),
            Err(Error::Interrupted)
        ));
    }
    assert_eq!(
        None,
        store
            .get(&answer.row.id, None)
            .unwrap()
            .row
            .wait_returned_at
    );
    assert_eq!(None, store.skip_reason(&answer).unwrap());
    assert!(waits(&temp.db()).is_empty());
}
