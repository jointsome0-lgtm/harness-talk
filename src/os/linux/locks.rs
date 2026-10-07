//! How the kernel's lock table names a locked file.
use std::{fs::Metadata, os::unix::fs::MetadataExt};

pub const LOCK_TABLE: &str = "/proc/locks";

/// A file as the lock table names it: device major, device minor, inode.
pub fn lock_key(m: &Metadata) -> (u64, u64, u64) {
    (
        u64::from(libc::major(m.dev())),
        u64::from(libc::minor(m.dev())),
        m.ino(),
    )
}
