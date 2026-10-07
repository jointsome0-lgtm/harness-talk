//! Linux: process groups held by pidfds, `/proc`, and file locks with the kernel's table of
//! who holds them.
mod capture;
mod group;
mod locks;
mod procfs;

pub use capture::run_command;
pub use group::{OwnedGroup, member_disappeared, parse_identity};
pub use locks::{LOCK_TABLE, lock, lock_key, try_lock};
pub use procfs::{PROC, process_exe, process_stat};
