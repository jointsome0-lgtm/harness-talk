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
    parse_identity(&fs::read(format!("/proc/{pid}/stat"))?)
}

fn parse_identity(stat: &[u8]) -> io::Result<Identity> {
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
fn member_disappeared(error: &io::Error) -> bool {
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
    fn reaped_owned_proc_entry_is_a_member_disappearance_but_not_a_valid_anchor() {
        use std::io::Read;
        let mut child = Command::new("/bin/true").process_group(0).spawn().unwrap();
        let group = OwnedGroup::new(child.id()).unwrap();
        let mut stat = fs::File::open(format!("/proc/{}/stat", child.id())).unwrap();
        child.wait().unwrap();
        let error = stat.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
        assert!(member_disappeared(&error));
        assert!(
            group.check_anchor().is_err(),
            "a disappeared anchor must fail closed"
        );
    }

    #[test]
    fn member_disappearance_does_not_hide_permission_or_other_io_failures() {
        for errno in [libc::ENOENT, libc::ESRCH] {
            assert!(member_disappeared(&io::Error::from_raw_os_error(errno)));
        }
        for errno in [libc::EPERM, libc::EACCES, libc::EIO] {
            assert!(!member_disappeared(&io::Error::from_raw_os_error(errno)));
        }
        assert!(!member_disappeared(&io::Error::other(
            "Invalid process stat"
        )));
    }

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
    fn stat_identity_accepts_non_utf8_comm_with_embedded_delimiters() {
        // comm is arbitrary bytes and can include parentheses or whitespace.
        // The final ')' separates it from the kernel's ASCII numeric fields.
        let mut stat = b"123 (odd\xff) name\n) S 1 123".to_vec();
        for _ in 3..19 {
            stat.extend_from_slice(b" 0");
        }
        stat.extend_from_slice(b" 456 0 0\n");
        let found = parse_identity(&stat).unwrap();
        assert_eq!((found.group, found.start), (123, 456));
        assert!(parse_identity(b"123 (odd\xff) S 1").is_err());
        assert!(parse_identity(b"123 (odd\xff S 1 123").is_err());
        let mut malformed = stat.clone();
        malformed.truncate(malformed.len() - b"456 0 0\n".len());
        malformed.extend_from_slice(b"\xff 0 0\n");
        assert!(parse_identity(&malformed).is_err());
    }

    fn stat_with_identity(group: &[u8], start: &[u8]) -> Vec<u8> {
        // Kernel stat field layout, with the signed group/session defaults
        // available when do_task_stat cannot lock an exiting task's sighand.
        let fields: &[&[u8]] = &[
            b"X",  // state (3)
            b"0",  // ppid (4)
            group, // pgrp (5), signed
            b"-1", // session (6)
            b"0",  // tty_nr (7)
            b"-1", // tpgid (8)
            b"0",  // flags (9)
            b"0",  // minflt (10)
            b"0",  // cminflt (11)
            b"0",  // majflt (12)
            b"0",  // cmajflt (13)
            b"0",  // utime (14)
            b"0",  // stime (15)
            b"0",  // cutime (16)
            b"0",  // cstime (17)
            b"20", // priority (18)
            b"0",  // nice (19)
            b"0",  // num_threads (20)
            b"0",  // itrealvalue (21)
            start, // starttime (22), unsigned
        ];
        let mut stat = b"123 (exiting fixture)".to_vec();
        for field in fields {
            stat.push(b' ');
            stat.extend_from_slice(field);
        }
        for _ in 23..=52 {
            stat.extend_from_slice(b" 0");
        }
        stat.push(b'\n');
        stat
    }

    #[test]
    fn stat_identity_accepts_signed_kernel_group_and_unsigned_starttime() {
        let found = parse_identity(&stat_with_identity(b"-1", b"456")).unwrap();
        assert_eq!((found.group, found.start), (-1, 456));
        for (group, start) in [(0, 0), (123, 456), (i32::MAX, u64::MAX)] {
            let found = parse_identity(&stat_with_identity(
                group.to_string().as_bytes(),
                start.to_string().as_bytes(),
            ))
            .unwrap();
            assert_eq!((found.group, found.start), (group, start));
        }
    }

    #[test]
    fn stat_identity_rejects_invalid_fields_without_truncation_or_record_contents() {
        for group in [
            b"bad".as_slice(),
            b"\xff",
            b"2147483648",
            b"-2147483649",
            // An unchecked u64-to-i32 cast would turn this into group 123.
            b"4294967419",
        ] {
            let error = parse_identity(&stat_with_identity(group, b"456"))
                .err()
                .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert!(error.to_string().contains("process stat pgrp:"));
            assert!(!error.to_string().contains("exiting fixture"));
            assert!(!member_disappeared(&error));
        }
        for start in [b"bad".as_slice(), b"\xff", b"-1", b"18446744073709551616"] {
            let error = parse_identity(&stat_with_identity(b"123", start))
                .err()
                .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert!(error.to_string().contains("process stat starttime:"));
            assert!(!error.to_string().contains("exiting fixture"));
            assert!(!member_disappeared(&error));
        }
        for (stat, field) in [
            (b"123 (private comm) X 0".as_slice(), "pgrp"),
            (b"123 (private comm) X 0 123".as_slice(), "starttime"),
        ] {
            let error = parse_identity(stat).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!("Incomplete process stat {field}")
            );
            assert!(!member_disappeared(&error));
        }
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
