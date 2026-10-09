//! A child run to its end inside a process group this program owns. Built where the Codex and
//! Claude Code adapters are, which run their client commands so; `build.rs` says where.
#![cfg(native_clients)]
use harness_talk::{error::Error, os};
use std::time::{Duration, Instant};

#[test]
fn a_descendant_holding_the_pipes_cannot_defeat_the_deadline() {
    let start = Instant::now();
    let result = os::run_command("/bin/sh", &["-c", "sleep 10 &"], Duration::from_millis(200));
    assert_eq!(result.unwrap_err(), Error::System("command_timed_out"));
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

/// The descendant is told apart from a later process of its number by `/proc`, and held by a
/// pidfd. On macOS `test_cli_compat` shows what a client command started ending with it.
#[cfg(target_os = "linux")]
#[test]
fn a_prompt_wrapper_exit_does_not_leave_a_term_ignoring_descendant() {
    use std::{
        fs,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };
    let record = std::env::temp_dir().join(format!("htalk-capture-{}", uuid::Uuid::new_v4()));
    let script = r#"import os, signal, sys, time
r,w=os.pipe()
pid=os.fork()
if pid==0:
    os.close(r)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.alarm(12)
    os.write(w,b'x')
    os.close(w)
    while True: time.sleep(.1)
os.close(w)
assert os.read(r,1)==b'x'
start=open('/proc/%d/stat'%pid).read().rpartition(')')[2].split()[19]
open(sys.argv[1],'w').write(str(pid)+' '+start)
"#;
    let start = Instant::now();
    let result = os::run_command(
        "/usr/bin/python3",
        &["-c", script, record.to_str().unwrap()],
        Duration::from_millis(200),
    );
    let recorded = fs::read_to_string(&record).unwrap();
    let (pid, started) = recorded.split_once(' ').unwrap();
    let pid: i32 = pid.parse().unwrap();
    fs::remove_file(record).unwrap();
    let same_process = || {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|stat| {
                stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19) == Some(started)
            })
    };
    // The hard-lived fixture is either still this recorded descendant or gone.
    // Pin it before the assertion so an unfixed test can dispose of its own leak.
    let fd = if same_process() {
        unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) }
    } else {
        -1
    };
    let stopped = if fd < 0 {
        if same_process() {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        true
    } else {
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(unsafe { libc::poll(&mut poll, 1, 0) } >= 0);
        let still_original = same_process();
        let exited = poll.revents & libc::POLLIN != 0 || !still_original;
        if still_original {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
        exited
    };
    assert_eq!(result.unwrap_err(), Error::System("command_timed_out"));
    assert!(start.elapsed() < Duration::from_secs(6));
    assert!(
        stopped,
        "the owned TERM-ignoring descendant survived command cleanup"
    );
}
