//! What holds on Windows of a process this program started and has not yet put in a job of
//! its own.
//!
//! This test reaches inside. No command leaves a child in that state for longer than a
//! fraction of a millisecond, so no command shows what happens to it.
#![cfg(windows)]
use harness_talk::os::Grouped;
use std::{
    env,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject},
};

const NAME: &str = "what_a_killed_process_started_ends_with_it";
const ROLE: &str = "HTALK_TEST_ROLE";

/// This executable again, as one of the two processes the test needs.
fn again(role: &str) -> Command {
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args(["--exact", NAME, "--nocapture"])
        .env(ROLE, role);
    command
}

#[test]
fn what_a_killed_process_started_ends_with_it() {
    match env::var(ROLE).as_deref() {
        // Lives for half a minute at most, should nothing end it.
        Ok("started") => return std::thread::sleep(Duration::from_secs(30)),
        // Starts a process as the program does, puts it in no job, and waits to be killed. The
        // started process is given nothing of the test's to hold open.
        Ok(_) => {
            return tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let started = tokio::process::Command::from(again("started"))
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .own_group()
                        .spawn()
                        .unwrap();
                    println!("started {}", started.id().unwrap());
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
        }
        Err(_) => (),
    }
    let mut parent = again("parent").stdout(Stdio::piped()).spawn().unwrap();
    // The test harness can put its own words before the number on the same line.
    let started = BufReader::new(parent.stdout.take().unwrap())
        .lines()
        .find_map(|line| line.unwrap().rsplit_once("started ")?.1.parse::<u32>().ok())
        .expect("the parent started a process");
    const SYNCHRONIZE: u32 = 0x0010_0000;
    // An open handle keeps the number from naming another process.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, started) };
    assert!(!handle.is_null(), "the started process is gone already");
    let runs = unsafe { WaitForSingleObject(handle, 200) };
    parent.kill().unwrap();
    parent.wait().unwrap();
    let ended = unsafe { WaitForSingleObject(handle, 10_000) };
    unsafe { CloseHandle(handle) };
    assert_eq!(WAIT_TIMEOUT, runs, "the started process did not run");
    assert_eq!(
        WAIT_OBJECT_0, ended,
        "the started process outlived the one that started it"
    );
}
