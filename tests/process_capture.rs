use harness_talk::{error::Failure, os};
use std::time::{Duration, Instant};

#[test]
fn a_descendant_holding_the_pipes_cannot_defeat_the_deadline() {
    let start = Instant::now();
    let result = os::run_command("/bin/sh", &["-c", "sleep 10 &"], Duration::from_millis(200));
    assert_eq!(result.unwrap_err(), Failure::Class("TimeoutExpired"));
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn captures_both_pipes_without_a_full_pipe_deadlock() {
    let result = os::run_command(
        "/usr/bin/python3",
        &[
            "-c",
            "import os; os.write(1,b'a'*200000); os.write(2,b'b'*200000)",
        ],
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, vec![b'a'; 200000]);
    assert_eq!(result.stderr, vec![b'b'; 200000]);
}
