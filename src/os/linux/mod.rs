//! Linux: process groups held by pidfds, `/proc`, the kernel's table of who holds a file
//! lock, and a socket connected within a time.
mod capture;
mod connect;
mod group;
mod locks;
mod procfs;

pub use capture::run_command;
pub use connect::connect_unix;
pub use group::{OwnedGroup, member_disappeared, parse_identity};
pub use locks::{LOCK_TABLE, lock_key};
pub use procfs::{PROC, process_exe, process_stat};
