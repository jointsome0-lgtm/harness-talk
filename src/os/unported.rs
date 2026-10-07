//! What a system other than Linux has not been given yet. A port replaces its item here with
//! the system's own.
use std::{io, path::Path};

/// No process table is read here, so no session is recognized by its ancestry.
pub const PROC: &str = "/proc";
pub fn process_stat(_root: &Path, _pid: i64) -> Option<(String, i64, String)> {
    None
}
pub fn process_exe(_root: &Path, _pid: i64) -> Option<Vec<u8>> {
    None
}

/// A child's process group. It has no value here, so nothing that holds one can run.
pub enum OwnedGroup {}
impl OwnedGroup {
    pub fn new(_pid: u32) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn exited(&self) -> io::Result<bool> {
        match *self {}
    }
    pub(crate) fn interrupt(&self) -> io::Result<()> {
        match *self {}
    }
    pub async fn finish_async(&mut self) -> io::Result<()> {
        match *self {}
    }
}
