//! Cleanup for a newly spawned private process group on macOS, which has no pidfds and no
//! `/proc`, and what the system says of a process there.
//!
//! As on Linux, the caller must retain the direct Child without polling wait/try_wait until
//! cleanup completes. A group is numbered by its leader, and the unreaped child keeps its
//! number, so until then the number names this group and no other. Members are not listed one
//! by one: the kernel says whether a signal to the group found a process that still runs.
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

/// macOS keeps no table of held file locks that a program can read.
pub const LOCK_TABLE: Option<&str> = None;

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
        } else if unknown.raw_os_error() == Some(libc::ESRCH) {
            // A child that has exited, or is on its way out, no longer says which group it
            // led. No other group can have its number while it is unreaped, and a child that
            // is not this process's to wait for is an error here.
            group.check_anchor()?;
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
    fn signal(&self, value: i32) -> io::Result<()> {
        if unsafe { libc::kill(self.leader, value) } == 0 {
            return Ok(());
        }
        match io::Error::last_os_error() {
            e if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
            e => Err(e),
        }
    }

    pub(crate) fn interrupt(&self) -> io::Result<()> {
        self.signal(libc::SIGINT)
    }

    // Used after a group cleanup that failed. It claims nothing of the descendants.
    pub(crate) fn kill_leader(&self) -> io::Result<()> {
        self.signal(libc::SIGKILL)
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
            // The kernel looks at the members it listed a moment before, so one that was started
            // meanwhile is missed once; the group is asked again before it is called empty.
            Some(libc::ESRCH | libc::EPERM) if exited => {
                Ok(unsafe { libc::kill(-self.leader, value) } != 0)
            }
            // The child is on its way out: the kernel no longer signals it and does not say
            // yet that it has exited. The group is not called empty before it does.
            Some(libc::ESRCH | libc::EPERM) => Ok(false),
            _ => Err(e),
        }
    }

    /// The same as `finish_async`, for a caller outside the async runtime.
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

/// macOS keeps no directory of its processes. The system is asked about one by its number, and
/// the root that Linux reads under is not looked at.
pub const PROC: &str = "";

/// One record of a process. False when the system does not give it.
fn record<T>(pid: libc::c_int, flavor: libc::c_int, into: &mut T) -> bool {
    let size = std::mem::size_of::<T>() as libc::c_int;
    // SAFETY: the system writes no more than `size` bytes into a record of that size.
    unsafe { libc::proc_pidinfo(pid, flavor, 0, std::ptr::from_mut(into).cast(), size) == size }
}

/// Comm, parent PID and start time of a process, or `None` when there is no such process.
/// The start is written as `ps -o lstart=` prints it in the C locale and in UTC, which is how
/// Claude Code records it on macOS. The record that holds it is refused for a process of
/// another account, and the start is empty then.
pub fn process_stat(_root: &Path, pid: i64) -> Option<(String, i64, String)> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut short: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    if !record(pid, libc::PROC_PIDT_SHORTBSDINFO, &mut short) {
        return None;
    }
    let mut full: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let start = if record(pid, libc::PROC_PIDTBSDINFO, &mut full) {
        started(full.pbi_start_tvsec)
    } else {
        String::new()
    };
    let comm: Vec<u8> = short.pbsi_comm.iter().map(|b| *b as u8).collect();
    let comm = comm.split(|b| *b == 0).next().unwrap_or_default();
    let comm = String::from_utf8_lossy(comm).into_owned();
    Some((comm, i64::from(short.pbsi_ppid), start))
}

/// The file name of the executable of a process, as bytes. A process started from a script
/// has the executable of its interpreter, and its comm is the interpreter's name too.
pub fn process_exe(_root: &Path, pid: i64) -> Option<Vec<u8>> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the system writes no more than the length of the buffer it is given.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    path.truncate(usize::try_from(length).ok().filter(|length| *length > 0)?);
    Some(path.rsplit(|b| *b == b'/').next().unwrap_or(&path).to_vec())
}

/// Seconds since 1970 as `Fri Oct  9 01:36:44 2026`, in UTC.
fn started(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (days, rest) = (seconds / 86400, seconds % 86400);
    // The date of a day counted from 1970, in years that begin with March.
    let shifted = days + 719_468;
    let (era, of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{} {} {day:>2} {:02}:{:02}:{:02} {year}",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}
