//! Fault-path check through run_command's production constructor error arm.
use super::run_command_with_group;
use crate::error::Failure;
use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    directory: PathBuf,
    process: Option<(i32, OwnedFd)>,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("htalk-constructor-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        Self {
            directory,
            process: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some((pid, fd)) = &self.process {
            // This descriptor pins only the owned fixture, even after reaping.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let waited = unsafe { libc::waitpid(*pid, std::ptr::null_mut(), libc::WNOHANG) };
                if waited != 0 {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn exited(fd: &OwnedFd) -> bool {
    let mut event = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        if unsafe { libc::poll(&mut event, 1, 0) } >= 0 {
            break;
        }
        assert_eq!(
            io::Error::last_os_error().kind(),
            io::ErrorKind::Interrupted
        );
    }
    assert_eq!(event.revents & (libc::POLLERR | libc::POLLNVAL), 0);
    event.revents & libc::POLLIN != 0
}

#[test]
fn ownership_constructor_failure_stops_direct_child_before_returning_original_error() {
    let mut fixture = Fixture::new();
    let ready = fixture.directory.join("ready");
    let release = fixture.directory.join("release");
    let effect = fixture.directory.join("effect");
    let script = r#"import signal, sys, time
from pathlib import Path
signal.alarm(12)
ready, release, effect = map(Path, sys.argv[1:])
ready.touch()
while not release.exists(): time.sleep(.01)
effect.write_text('owned harmless side effect')
"#;
    let mut constructor_called = false;
    let mut failure_started = None;
    let result = run_command_with_group(
        "/usr/bin/python3",
        &[
            "-c",
            script,
            ready.to_str().unwrap(),
            release.to_str().unwrap(),
            effect.to_str().unwrap(),
        ],
        Duration::from_secs(10),
        |pid| {
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                // Let the production error arm dispose of the retained Child
                // before any assertion can fail without the pidfd guard.
                return Err(io::Error::last_os_error());
            }
            fixture.process = Some((pid as i32, unsafe { OwnedFd::from_raw_fd(fd as i32) }));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready.exists() {
                assert!(
                    Instant::now() < deadline,
                    "owned child did not reach its gate"
                );
                thread::sleep(Duration::from_millis(5));
            }
            assert!(
                !exited(&fixture.process.as_ref().unwrap().1),
                "constructor was not called with a live child"
            );
            constructor_called = true;
            failure_started = Some(Instant::now());
            Err(io::Error::from_raw_os_error(libc::EACCES))
        },
    );
    assert!(constructor_called);
    assert_eq!(result.unwrap_err(), Failure::Class("PermissionError"));
    assert!(failure_started.unwrap().elapsed() < Duration::from_secs(3));
    let stopped_before_return = exited(&fixture.process.as_ref().unwrap().1);
    fs::write(&release, b"release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !effect.exists()
        && !exited(&fixture.process.as_ref().unwrap().1)
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        stopped_before_return,
        "owned child survived constructor failure; harmless later side effect observed={}",
        effect.exists()
    );
    assert!(
        !effect.exists(),
        "owned direct child wrote after cleanup returned"
    );
    assert_eq!(
        unsafe {
            libc::waitpid(
                fixture.process.as_ref().unwrap().0,
                std::ptr::null_mut(),
                libc::WNOHANG,
            )
        },
        -1,
        "direct child was not reaped before returning"
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
