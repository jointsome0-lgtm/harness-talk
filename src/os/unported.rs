//! What a system other than Linux has not been given yet. A port replaces its item here with
//! the system's own.
use std::path::Path;

/// No process table is read here, so no session is recognized by its ancestry.
pub const PROC: &str = "/proc";
pub fn process_stat(_root: &Path, _pid: i64) -> Option<(String, i64, String)> {
    None
}
pub fn process_exe(_root: &Path, _pid: i64) -> Option<Vec<u8>> {
    None
}
