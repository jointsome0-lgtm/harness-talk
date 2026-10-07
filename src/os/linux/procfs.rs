//! What the program reads of another process under `/proc`. The root is a parameter, so a
//! test can put a directory of its own there.
use std::{os::unix::ffi::OsStrExt, path::Path};

pub const PROC: &str = "/proc";

/// Comm, parent PID and start time from `PID/stat`, or `None` when the entry cannot be read
/// or parsed.
pub fn process_stat(root: &Path, pid: i64) -> Option<(String, i64, String)> {
    let bytes = std::fs::read(root.join(pid.to_string()).join("stat")).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    // comm may contain spaces and parentheses; the fields after the last ")" cannot.
    let (head, tail) = text.rsplit_once(')').unwrap_or(("", &text));
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let parent = fields.get(1)?.parse::<i64>().ok()?;
    let started = fields.get(19)?.to_string();
    let comm = head.split_once('(').map_or("", |(_, comm)| comm).to_owned();
    Some((comm, parent, started))
}

/// The file name of the executable `PID/exe` points to, as bytes.
pub fn process_exe(root: &Path, pid: i64) -> Option<Vec<u8>> {
    let exe = std::fs::read_link(root.join(pid.to_string()).join("exe")).ok()?;
    let exe = exe.as_os_str().as_bytes();
    Some(exe.rsplit(|b| *b == b'/').next().unwrap_or(exe).to_vec())
}
