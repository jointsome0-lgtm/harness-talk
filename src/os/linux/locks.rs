//! Advisory locks on whole files, and how the kernel's lock table names a locked file.
use std::{
    fs::{File, Metadata},
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

pub const LOCK_TABLE: &str = "/proc/locks";

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
/// A file as the lock table names it: device major, device minor, inode.
pub fn lock_key(m: &Metadata) -> (u64, u64, u64) {
    (
        u64::from(libc::major(m.dev())),
        u64::from(libc::minor(m.dev())),
        m.ino(),
    )
}
