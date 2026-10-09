//! The kernel's table of who holds a file lock.

pub const LOCK_TABLE: Option<&str> = Some("/proc/locks");
