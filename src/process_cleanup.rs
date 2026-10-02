//! Cleanup for a newly spawned private process group.
//!
//! The caller must retain the direct Child without polling wait/try_wait until
//! cleanup completes. Its unreaped PID reserves the group number. Signals use
//! pidfds for individually inspected members, never a saved numeric group ID.
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
struct Identity {
    group: i32,
    start: u64,
}

fn identity(pid: i32) -> io::Result<Identity> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("Invalid process stat"))?
        .1
        .split_whitespace()
        .collect();
    let parse = |index: usize| -> io::Result<u64> {
        fields
            .get(index)
            .ok_or_else(|| io::Error::other("Incomplete process stat"))?
            .parse()
            .map_err(io::Error::other)
    };
    Ok(Identity {
        group: parse(2)? as i32,
        start: parse(19)?,
    })
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

pub(crate) struct OwnedGroup {
    leader: i32,
    identity: Identity,
    leader_fd: OwnedFd,
    members: HashMap<(i32, u64), OwnedFd>,
}

impl OwnedGroup {
    pub(crate) fn new(pid: u32) -> io::Result<Self> {
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

    fn check_anchor(&self) -> io::Result<()> {
        if identity(self.leader)? != self.identity {
            return Err(io::Error::other(
                "Process group anchor changed; cleanup not confirmed",
            ));
        }
        Ok(())
    }

    // poll observes exit without reaping, including handles above FD_SETSIZE.
    pub(crate) fn exited(&self) -> io::Result<bool> {
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

    fn step(&mut self, value: i32) -> io::Result<bool> {
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
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if found.group != self.leader {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
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
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
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

    pub(crate) async fn finish_async(&mut self) -> io::Result<()> {
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };

    #[test]
    fn retained_tokio_wrapper_is_not_reaped_by_the_signal_driver() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut child = tokio::process::Command::new("/bin/true")
                .process_group(0)
                .spawn()
                .unwrap();
            let mut group = OwnedGroup::new(child.id().unwrap()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !group.exited().unwrap() {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            group.check_anchor().unwrap();
            group.finish_async().await.unwrap();
            assert!(child.wait().await.unwrap().success());
            assert!(group.step(libc::SIGTERM).is_err());
        });
    }

    #[test]
    fn high_process_handles_and_reaped_anchor_fail_closed() {
        let files: Vec<_> = (0..1100)
            .map(|_| fs::File::open("/dev/null").unwrap())
            .collect();
        let mut child = Command::new("/bin/true")
            .stdout(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = OwnedGroup::new(child.id()).unwrap();
        assert!(group.leader_fd.as_raw_fd() >= 1024);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !group.exited().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(child.wait().unwrap().success());
        let mut group = group;
        assert!(group.step(libc::SIGKILL).is_err());
        drop(files);
    }

    #[test]
    fn changed_anchor_is_rejected_before_any_signal() {
        let mut child = Command::new("/bin/sleep")
            .arg("10")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut group = OwnedGroup::new(child.id()).unwrap();
        group.identity.start = group.identity.start.wrapping_add(1);
        let rejected = group.step(libc::SIGKILL).is_err();
        let untouched = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(rejected);
        assert!(untouched, "ownership rejection still signalled the child");
    }
}
