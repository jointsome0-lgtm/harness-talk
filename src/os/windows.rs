//! Windows: where the mailbox is, and how its files are opened. Interrupts, file locks and a
//! closed output are not ported yet; each says below what happens until then.
use super::home;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub const DB_HELP: &str = "Shared SQLite file. Default: HTALK_DB, then %LOCALAPPDATA%\\harness-talk\\mail.sqlite3. Only peer add creates a missing file.";

/// The mailbox when `--db` names none, as `DB_HELP` says.
pub fn default_db() -> PathBuf {
    if let Some(db) = env::var_os("HTALK_DB").filter(|v| !v.is_empty()) {
        return db.into();
    }
    env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join("AppData").join("Local"), PathBuf::from)
        .join("harness-talk")
        .join("mail.sqlite3")
}

/// Not ported: Ctrl-C ends the process the way Windows does by default.
pub fn install_interrupt_handler() -> io::Result<()> {
    Ok(())
}
pub fn interrupted() -> bool {
    false
}

/// Ctrl-C, then the console closing, as streams for an async runtime.
pub fn stop_requests() -> io::Result<(
    tokio::signal::windows::CtrlC,
    tokio::signal::windows::CtrlClose,
)> {
    use tokio::signal::windows::{ctrl_c, ctrl_close};
    Ok((ctrl_c()?, ctrl_close()?))
}

/// A child that leads a process group of its own. Not ported: the child stays in this one.
pub trait Grouped {
    fn own_group(&mut self) -> &mut Self;
}
impl Grouped for tokio::process::Command {
    fn own_group(&mut self) -> &mut Self {
        self
    }
}

/// A file is as private as the directory it is created in; no access list is changed.
pub fn private_umask() {}
/// How a file is opened where the portable options do not reach.
pub trait Open {
    fn private(&mut self) -> &mut Self;
    fn no_follow(&mut self) -> &mut Self;
}
impl Open for OpenOptions {
    fn private(&mut self) -> &mut Self {
        self
    }
    fn no_follow(&mut self) -> &mut Self {
        self
    }
}
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}
/// Windows gives no way to flush a directory. A file is flushed itself before it is renamed.
pub fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Not ported: without a lock on a directory, contended writers queue in SQLite alone.
pub fn open_directory(_path: &Path) -> io::Result<File> {
    Err(io::ErrorKind::Unsupported.into())
}
pub fn lock(_file: &File) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}
/// Not ported: a closed output is noticed at the next write.
pub fn hung_up<T>(_io: &T) -> bool {
    false
}
