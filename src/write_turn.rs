//! Admission for contended sends before their first write transaction.
use crate::{error::Error, os};
use std::cell::Cell;
use std::fs::File;
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
}
/// What a busy mailbox means to a send before its first write: `None` is the wait of any
/// writer, `Some(false)` is no wait, and `Some(true)` is the same budget in pauses of one
/// millisecond, for the send that holds the turn.
pub fn wait() -> Option<bool> {
    match MODE.get() {
        Mode::Normal => None,
        Mode::Fresh => Some(false),
        // Nothing is written yet, so an interrupt ends the wait. Later waits record what is
        // already known, a notification's receipt among it, and are not cut short.
        Mode::Heir => Some(!os::interrupted()),
    }
}

// A timed-out lock request can remain blocked. Bound that cost to one helper per
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
    if let Err(error) = os::create_private_dir(Path::new(&name))
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Ok(None);
    }
    let Ok(file) = os::open_directory(Path::new(&name)) else {
        return Ok(None);
    };
    let (send, receive) = mpsc::sync_channel(1);
    if thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            let _waiting = waiting;
            let result = loop {
                match os::lock(&file) {
                    Ok(()) => break Some(file),
                    Err(error) if error.kind() != io::ErrorKind::Interrupted => break None,
                    Err(_) => (),
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
