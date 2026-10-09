//! Unix, which here is Linux and macOS: signals, sockets, file locks, who owns a file and what
//! a descriptor is ready for.
use super::data_home;
use crate::error::Error;
use std::{
    env,
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub use std::os::fd::AsRawFd as Descriptor;
pub use std::os::unix::net::UnixStream as Socket;

/// Where the system keeps its OpenSSH client.
pub const SSH: &str = "/usr/bin/ssh";

/// The error numbers the program tells apart.
pub mod errno {
    pub use libc::{
        EACCES, EAGAIN, EALREADY, EBADF, ECHILD, ECONNABORTED, ECONNREFUSED, ECONNRESET, EEXIST,
        EINPROGRESS, EINTR, EISDIR, ELOOP, ENOENT, ENOTDIR, EPERM, EPIPE, ESHUTDOWN, ESRCH,
        ETIMEDOUT,
    };
}

#[cfg(not(target_os = "macos"))]
pub const DB_HELP: &str = "Shared SQLite file. Default: HTALK_DB, then $XDG_DATA_HOME/harness-talk/mail.sqlite3, then ~/.local/share/harness-talk/mail.sqlite3. Only peer add creates a missing file.";
#[cfg(target_os = "macos")]
pub const DB_HELP: &str = "Shared SQLite file. Default: HTALK_DB, then $XDG_DATA_HOME/harness-talk/mail.sqlite3, then ~/Library/Application Support/harness-talk/mail.sqlite3. Only peer add creates a missing file.";

/// The mailbox when `--db` names none, as `DB_HELP` says.
pub fn default_db() -> PathBuf {
    if let Some(db) = env::var_os("HTALK_DB").filter(|v| !v.is_empty()) {
        return db.into();
    }
    #[cfg(target_os = "macos")]
    if env::var_os("XDG_DATA_HOME").is_none_or(|v| v.is_empty()) {
        return super::home().join("Library/Application Support/harness-talk/mail.sqlite3");
    }
    data_home().join("harness-talk/mail.sqlite3")
}

static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
pub fn install_interrupt_handler() -> io::Result<()> {
    let flag = INTERRUPTED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    signal_hook::flag::register(signal_hook::consts::SIGINT, flag)?;
    Ok(())
}
pub fn interrupted() -> bool {
    INTERRUPTED.get().is_some_and(|f| f.load(Ordering::Relaxed))
}

/// While it lives, an interrupt or a request to terminate sets the flag instead of ending the
/// process. Dropping it gives both back and clears the flag.
pub struct Stop(Vec<signal_hook::SigId>, Arc<AtomicBool>);
impl Stop {
    pub fn on(flag: Arc<AtomicBool>) -> io::Result<Self> {
        let signals = vec![
            signal_hook::flag::register(signal_hook::consts::SIGINT, flag.clone())?,
            signal_hook::flag::register(signal_hook::consts::SIGTERM, flag.clone())?,
        ];
        Ok(Self(signals, flag))
    }
}
impl Drop for Stop {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        self.1.store(false, Ordering::Relaxed);
    }
}

/// The two requests to stop, as streams for an async runtime: an interrupt, then a request
/// to terminate.
pub fn stop_requests() -> io::Result<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)> {
    use tokio::signal::unix::{SignalKind, signal};
    Ok((
        signal(SignalKind::interrupt())?,
        signal(SignalKind::terminate())?,
    ))
}

/// A child that leads a process group of its own.
pub trait Grouped {
    fn own_group(&mut self) -> &mut Self;
}
impl Grouped for std::process::Command {
    fn own_group(&mut self) -> &mut Self {
        self.process_group(0)
    }
}
impl Grouped for tokio::process::Command {
    fn own_group(&mut self) -> &mut Self {
        self.process_group(0)
    }
}
/// Kills what is left of a child's process group, named by its number.
pub fn kill_group(group: i32) {
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
}
/// From here on, what this process creates is for its owner alone unless a mode says more.
pub fn private_umask() {
    unsafe {
        libc::umask(0o077);
    }
}

/// How a file is opened where the portable options do not reach.
pub trait Open {
    /// A file this call creates is readable and writable by its owner alone.
    fn private(&mut self) -> &mut Self;
    /// A symbolic link as the last part of the path is refused.
    fn no_follow(&mut self) -> &mut Self;
}
impl Open for OpenOptions {
    fn private(&mut self) -> &mut Self {
        self.mode(0o600)
    }
    fn no_follow(&mut self) -> &mut Self {
        self.custom_flags(libc::O_NOFOLLOW)
    }
}
/// Creates one directory for its owner alone.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}
/// Makes a directory's entries, a new or renamed file among them, last through a power loss.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
/// Opens a directory itself, never a link to one.
pub fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

/// Owned by the account that runs this process.
pub fn is_mine(m: &Metadata) -> bool {
    m.uid() == unsafe { libc::getuid() }
}
pub fn is_roots(m: &Metadata) -> bool {
    m.uid() == 0
}
/// Mine, and no one else may read, write or enter it.
pub fn is_private(m: &Metadata) -> bool {
    is_mine(m) && m.mode() & 0o077 == 0
}
/// Its group or anyone else may write it.
pub fn others_write(m: &Metadata) -> bool {
    m.mode() & 0o022 != 0
}
pub fn is_executable(m: &Metadata) -> bool {
    m.mode() & 0o111 != 0
}
pub fn is_socket(m: &Metadata) -> bool {
    m.file_type().is_socket()
}
/// A file as a table of held locks names it: device major, device minor, inode.
pub fn lock_key(m: &Metadata) -> (u64, u64, u64) {
    let device = m.dev() as libc::dev_t;
    (
        libc::major(device) as u64,
        libc::minor(device) as u64,
        m.ino(),
    )
}
/// Device and inode: the same file under any name.
pub fn file_id(m: &Metadata) -> (u64, u64) {
    (m.dev(), m.ino())
}
/// How many names the file has.
pub fn links(m: &Metadata) -> u64 {
    m.nlink()
}
/// When its inode last changed, in seconds and nanoseconds.
pub fn changed(m: &Metadata) -> (i64, i64) {
    (m.ctime(), m.ctime_nsec())
}
/// Interface flags that say it is up and can send multicast.
pub fn multicast_ready(flags: u32) -> bool {
    flags & libc::IFF_UP as u32 != 0 && flags & libc::IFF_MULTICAST as u32 != 0
}

/// Asks for the lock without waiting. False when another holder has it.
pub fn try_lock(file: &File, shared: bool) -> io::Result<bool> {
    let mode = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}
/// Waits for the exclusive lock. An interrupted wait is an error like any other.
pub fn lock(file: &File) -> io::Result<()> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn set_nonblocking(io: &impl AsRawFd) -> io::Result<()> {
    let fd = io.as_raw_fd();
    // SAFETY: fcntl operates on a live descriptor the caller owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
/// Waits up to `wait` for `io` to be readable, or writable. False when the time passed.
pub fn ready(io: &impl AsRawFd, writable: bool, wait: Duration) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: io.as_raw_fd(),
        events: if writable {
            libc::POLLOUT
        } else {
            libc::POLLIN
        },
        revents: 0,
    };
    let milliseconds = wait.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32;
    // SAFETY: polls one descriptor owned by the caller for the duration of this call.
    match unsafe { libc::poll(&mut poll, 1, milliseconds) } {
        0 => Ok(false),
        n if n < 0 => Err(io::Error::last_os_error()),
        _ => Ok(true),
    }
}
/// The other end of `io` is gone, seen without reading or writing.
pub fn hung_up(io: &impl AsRawFd) -> bool {
    let mut sink = libc::pollfd {
        fd: io.as_raw_fd(),
        events: 0,
        revents: 0,
    };
    // macOS reports only on what it is asked for, and a hang-up is a thing of a pipe or a
    // socket: a device may read as an invalid descriptor there while it is open.
    #[cfg(target_os = "macos")]
    {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let known = unsafe { libc::fstat(sink.fd, &mut stat) } == 0;
        let kind = stat.st_mode & libc::S_IFMT;
        if known && kind != libc::S_IFIFO && kind != libc::S_IFSOCK {
            return false;
        }
        sink.events = libc::POLLOUT;
    }
    let seen = unsafe { libc::poll(&mut sink, 1, 0) };
    seen > 0 && sink.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
}

pub fn owned_socket(path: &Path) -> Result<PathBuf, Error> {
    let metadata = fs::metadata(path)?;
    if !is_socket(&metadata) || !is_mine(&metadata) {
        return Err(Error::code("recipient_socket_unavailable"));
    }
    Ok(path.to_path_buf())
}
