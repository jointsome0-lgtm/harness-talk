//! Cleanup for a newly spawned private process group.
//!
//! The caller must retain the direct Child without polling wait/try_wait until
//! cleanup completes. Its unreaped PID reserves the group number. Signals use
//! pidfds for individually inspected members, never a saved numeric group ID.
//!
//! The items that are `pub` are so for `tests/process_ownership.rs` only.
use std::{
    collections::HashMap,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub group: i32,
    pub start: u64,
}

fn identity(pid: i32) -> io::Result<Identity> {
    parse_identity(&fs::read(format!("/proc/{pid}/stat"))?)
}

pub fn parse_identity(stat: &[u8]) -> io::Result<Identity> {
    // Linux comm is arbitrary bytes and can include ')'. Only the tail
    // after its final delimiter contains the ASCII fields needed here.
    let end = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| io::Error::other("Invalid process stat"))?;
    let fields: Vec<_> = stat[end + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    fn parse<T: std::str::FromStr>(fields: &[&[u8]], index: usize, name: &str) -> io::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        let field = fields
            .get(index)
            .ok_or_else(|| io::Error::other(format!("Incomplete process stat {name}")))?;
        std::str::from_utf8(field)
            .map_err(|e| io::Error::other(format!("Invalid process stat {name}: {e}")))?
            .parse()
            .map_err(|e| io::Error::other(format!("Invalid process stat {name}: {e}")))
    }
    Ok(Identity {
        // Linux emits signed pgrp (including -1 during task teardown), while
        // starttime is unsigned. Parse directly into their identity types.
        group: parse::<i32>(&fields, 2, "pgrp")?,
        start: parse::<u64>(&fields, 19, "starttime")?,
    })
}

// A proc entry can disappear after open but before read, which Linux reports as
// ESRCH rather than ENOENT. This is safe to skip only for enumerated members;
// the retained group anchor must still match exactly.
pub fn member_disappeared(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

fn pin(pid: i32) -> io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        let fd = loop {
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd >= 0 {
                break fd;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        };
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Owned group cleanup requires Linux pidfds and /proc",
        ))
    }
}

fn alive(fd: &OwnedFd) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        if unsafe { libc::poll(&mut poll, 1, 0) } >= 0 {
            break;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    if poll.revents & (libc::POLLNVAL | libc::POLLERR) != 0 {
        return Err(io::Error::other("Invalid process handle"));
    }
    Ok(poll.revents == 0)
}

fn signal(fd: &OwnedFd, value: i32) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        loop {
            if unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    value,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            } >= 0
            {
                break;
            }
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (fd, value);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Owned group cleanup requires Linux pidfds",
        ))
    }
}

pub struct OwnedGroup {
    leader: i32,
    pub identity: Identity,
    leader_fd: OwnedFd,
    members: HashMap<(i32, u64), OwnedFd>,
}

impl OwnedGroup {
    pub fn new(pid: u32) -> io::Result<Self> {
        let leader = i32::try_from(pid).map_err(io::Error::other)?;
        let expected = identity(leader)?;
        if expected.group != leader {
            return Err(io::Error::other("Child does not own its process group"));
        }
        let leader_fd = pin(leader)?;
        if identity(leader)? != expected {
            return Err(io::Error::other("Process ownership changed"));
        }
        Ok(Self {
            leader,
            identity: expected,
            leader_fd,
            members: HashMap::new(),
        })
    }

    pub fn check_anchor(&self) -> io::Result<()> {
        if identity(self.leader)? != self.identity {
            return Err(io::Error::other(
                "Process group anchor changed; cleanup not confirmed",
            ));
        }
        Ok(())
    }

    // poll observes exit without reaping, including handles above FD_SETSIZE.
    pub fn exited(&self) -> io::Result<bool> {
        Ok(!alive(&self.leader_fd)?)
    }

    // Used after an explicit group cleanup failure. This only targets the
    // pinned wrapper, and does not claim that descendants have stopped.
    pub(crate) fn kill_leader(&self) -> io::Result<()> {
        signal(&self.leader_fd, libc::SIGKILL)
    }

    pub(crate) fn interrupt(&self) -> io::Result<()> {
        signal(&self.leader_fd, libc::SIGINT)
    }

    pub fn step(&mut self, value: i32) -> io::Result<bool> {
        self.check_anchor()?;
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|v| v.parse::<i32>().ok())
            else {
                continue;
            };
            let found = match identity(pid) {
                Ok(found) => found,
                Err(e) if member_disappeared(&e) => continue,
                Err(e) => return Err(e),
            };
            if found.group != self.leader {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(e) if member_disappeared(&e) => continue,
                Err(e) => return Err(e),
            };
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::other(
                    "Process group member has a different owner",
                ));
            }
            let key = (pid, found.start);
            if self.members.contains_key(&key) {
                continue;
            }
            let fd = match pin(pid) {
                Ok(fd) => fd,
                Err(e) if e.raw_os_error() == Some(libc::ESRCH) => continue,
                Err(e) => return Err(e),
            };
            // A member may disappear between enumeration and pidfd_open. Do
            // not signal unless both the pinned process and anchor still match.
            match identity(pid) {
                Ok(current) if current == found => {
                    self.members.insert(key, fd);
                }
                Ok(_) => continue,
                Err(e) if member_disappeared(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        self.check_anchor()?;
        let mut active = false;
        for fd in self.members.values() {
            if alive(fd)? {
                signal(fd, value)?;
                active = true;
            }
        }
        Ok(!active)
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
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
            std::thread::sleep(Duration::from_millis(20));
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
