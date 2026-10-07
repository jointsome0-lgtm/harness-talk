//! Cleanup for a newly spawned private process group on macOS, which has no pidfds and no
//! `/proc`.
//!
//! As on Linux, the caller must retain the direct Child without polling wait/try_wait until
//! cleanup completes. A group is numbered by its leader, and the unreaped child keeps its
//! number, so until then the number names this group and no other. Members are not listed one
//! by one: the kernel says whether a signal to the group found a process that still runs.
use std::{
    io,
    time::{Duration, Instant},
};

pub struct OwnedGroup {
    leader: i32,
}

impl OwnedGroup {
    pub fn new(pid: u32) -> io::Result<Self> {
        let group = Self {
            leader: i32::try_from(pid).map_err(io::Error::other)?,
        };
        let found = unsafe { libc::getpgid(group.leader) };
        let unknown = io::Error::last_os_error();
        if found == group.leader {
            Ok(group)
        } else if found != -1 {
            Err(io::Error::other("Child does not own its process group"))
        } else if group.exited()? {
            // A child that has exited no longer says which group it led. No other group can
            // have its number while it is unreaped.
            Ok(group)
        } else {
            Err(unknown)
        }
    }

    /// The child is still this process's to wait for, so its number is still its own.
    pub fn check_anchor(&self) -> io::Result<()> {
        self.exited().map(drop)
    }

    /// Whether the child has exited. The question does not reap it.
    pub fn exited(&self) -> io::Result<bool> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        loop {
            let id = self.leader as libc::id_t;
            if unsafe { libc::waitid(libc::P_PID, id, &mut info, options) } == 0 {
                // Nothing is written while the child runs. Some versions of the system also
                // report a child that only stopped, which has not exited.
                return Ok(info.si_signo == libc::SIGCHLD
                    && !matches!(info.si_code, libc::CLD_STOPPED | libc::CLD_CONTINUED));
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    // This only targets the retained wrapper, whose number is its own until it is reaped.
    pub(crate) fn interrupt(&self) -> io::Result<()> {
        if unsafe { libc::kill(self.leader, libc::SIGINT) } == 0 {
            return Ok(());
        }
        match io::Error::last_os_error() {
            e if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
            e => Err(e),
        }
    }

    /// Signals every member that still runs. True when none does.
    pub fn step(&mut self, value: i32) -> io::Result<bool> {
        // A reaped child is an error here: its number may name another group by now.
        let exited = self.exited()?;
        if unsafe { libc::kill(-self.leader, value) } == 0 {
            return Ok(false);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            // The answers for a group in which only exited processes are left. A member that
            // belongs to another user reads the same, and is not told apart from them.
            Some(libc::ESRCH | libc::EPERM) if exited => Ok(true),
            _ => Err(e),
        }
    }

    pub async fn finish_async(&mut self) -> io::Result<()> {
        let start = Instant::now();
        loop {
            let value = if start.elapsed() < Duration::from_secs(2) {
                libc::SIGTERM
            } else {
                libc::SIGKILL
            };
            if self.step(value)? {
                return Ok(());
            }
            if start.elapsed() >= Duration::from_secs(4) {
                return Err(io::Error::other(
                    "Owned process cleanup did not complete in four seconds",
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
