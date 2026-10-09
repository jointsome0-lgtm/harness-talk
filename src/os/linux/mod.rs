//! Linux: process groups held by pidfds, `/proc` and the kernel's table of who holds a file
//! lock.
mod group;
mod locks;
mod procfs;

pub use group::{OwnedGroup, member_disappeared, parse_identity};
pub use locks::LOCK_TABLE;
pub use procfs::{PROC, process_exe, process_stat};
