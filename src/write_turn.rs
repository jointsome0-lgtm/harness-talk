//! Admission for contended sends before their first write transaction.
use crate::{error::Error, os};
use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::os::{
    fd::AsRawFd,
    unix::fs::{DirBuilderExt, OpenOptionsExt},
};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use std::{io, thread};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Fresh,
    Heir,
}
thread_local! { static MODE: Cell<Mode> = const { Cell::new(Mode::Normal) }; }
thread_local! { static HEIR_WAITS: Cell<u64> = const { Cell::new(0) }; }

pub struct Probe(Mode);
impl Drop for Probe {
    fn drop(&mut self) {
        MODE.set(self.0);
    }
}
pub fn probe() -> Probe {
    Probe(MODE.replace(Mode::Fresh))
}
pub fn fresh() -> bool {
    MODE.get() == Mode::Fresh
}
pub fn started_write() {
    MODE.set(Mode::Normal);
}
pub fn queued(held: bool) {
    MODE.set(if held { Mode::Heir } else { Mode::Normal });
    if held {
        successor_grace();
    }
}
pub fn successor_grace() {
    thread::sleep(Duration::from_millis(4));
}
pub fn extra_successor_slot() -> bool {
    uuid::Uuid::new_v4().as_bytes()[0] < 171
}
pub fn heir_waits() -> u64 {
    HEIR_WAITS.get()
}
pub fn wait(attempt: i32) -> Option<bool> {
    match MODE.get() {
        Mode::Normal => None,
        Mode::Fresh => Some(false),
        Mode::Heir if attempt >= 5000 => Some(false),
        Mode::Heir => {
            HEIR_WAITS.set(HEIR_WAITS.get().saturating_add(1));
            thread::sleep(Duration::from_millis(1));
            Some(true)
        }
    }
}

// A timed-out flock can remain blocked. Bound that cost to one helper per
// process; other acquisitions fall back to SQLite while it remains blocked.
static WAITING: AtomicBool = AtomicBool::new(false);

struct Waiting;
impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING.store(false, Ordering::Release);
    }
}

pub fn acquire(path: &Path) -> Result<Option<File>, Error> {
    if WAITING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Ok(None);
    }
    let waiting = Waiting;
    let mut name = os::resolve(path).as_os_str().to_os_string();
    name.push("-htalk-turn");
    // A directory can never alias a SQLite database file. Closing an auxiliary
    // descriptor must not release that database's process-wide POSIX locks.
    if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(&name)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Ok(None);
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(name)
    {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let (send, receive) = mpsc::sync_channel(1);
    if thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            let _waiting = waiting;
            let result = loop {
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                    break Some(file);
                }
                if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break None;
                }
            };
            // Dropping the disconnected result releases an acquired lock after
            // its caller has timed out or been interrupted.
            let _ = send.send(result);
        })
        .is_err()
    {
        return Ok(None);
    }
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if os::interrupted() {
            return Err(Error::Interrupted);
        }
        let Some(remaining) = until.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        match receive.recv_timeout(remaining.min(Duration::from_millis(25))) {
            Ok(file) => return Ok(file),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
    }
}
