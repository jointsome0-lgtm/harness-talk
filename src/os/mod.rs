//! What the program asks of the operating system. Nothing outside this directory names a
//! system or one of its facilities. `unix` holds what Linux and macOS share: signals, sockets,
//! file locks, file ownership and descriptors. `linux` holds owned process groups, `/proc` and
//! the lock table. `macos` holds its own owned process groups. `windows` holds what the mailbox
//! needs there. `unported` stands where a system has no port of a part yet; `build.rs` says
//! which parts a system has.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(not(target_os = "linux"))]
mod unported;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{
    LOCK_TABLE, OwnedGroup, PROC, connect_unix, lock_key, member_disappeared, parse_identity,
    process_exe, process_stat, run_command,
};
#[cfg(target_os = "macos")]
pub use macos::OwnedGroup;
#[cfg(unix)]
pub use unix::{
    DB_HELP, Descriptor, Grouped, Open, SSH, Socket, Stop, changed, create_private_dir, default_db,
    errno, file_id, hung_up, install_interrupt_handler, interrupted, is_executable, is_mine,
    is_private, is_roots, is_socket, kill_group, links, lock, multicast_ready, open_directory,
    others_write, owned_socket, private_umask, ready, set_nonblocking, stop_requests,
    sync_directory, try_lock,
};
#[cfg(not(target_os = "linux"))]
pub use unported::{PROC, process_exe, process_stat};
#[cfg(windows)]
pub use windows::{
    DB_HELP, Grouped, Open, OwnedGroup, create_private_dir, default_db, hung_up,
    install_interrupt_handler, interrupted, lock, open_directory, private_umask, stop_requests,
    sync_directory,
};

use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn home() -> PathBuf {
    match env::var_os("HOME") {
        Some(value) if value.is_empty() => PathBuf::from("/"),
        Some(value) => PathBuf::from(value),
        None => env::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}
/// `$XDG_DATA_HOME`, or `~/.local/share` when it is unset or empty.
pub fn data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join(".local/share"), PathBuf::from)
}
pub fn expand_user(path: &Path) -> PathBuf {
    if path == Path::new("~") {
        home()
    } else if let Ok(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        path.to_path_buf()
    }
}
pub fn resolve(path: &Path) -> PathBuf {
    fn walk(path: &Path, depth: usize) -> PathBuf {
        let mut out = if path.is_absolute() {
            PathBuf::new()
        } else {
            env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        };
        for part in path.components() {
            match part {
                // A drive or a share, on a system that has them, and then its root.
                Component::Prefix(prefix) => out = PathBuf::from(prefix.as_os_str()),
                Component::RootDir => out.push(part.as_os_str()),
                Component::CurDir => (),
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(p) => {
                    out.push(p);
                    if depth < 40
                        && let Ok(target) = fs::read_link(&out)
                    {
                        let target = if target.is_absolute() {
                            target
                        } else {
                            out.parent().unwrap_or(Path::new("/")).join(target)
                        };
                        out = walk(&target, depth + 1);
                    }
                }
            }
        }
        out
    }
    walk(&expand_user(path), 0)
}
pub fn resolve_strict(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(expand_user(path))
}
/// Compare a native path with the canonical workspace saved by registration.
pub fn same_workspace(directory: Option<&serde_json::Value>, workspace: &str) -> bool {
    match directory {
        Some(serde_json::Value::String(directory)) if Path::new(directory).is_absolute() => {
            resolve(Path::new(directory)).as_os_str() == std::ffi::OsStr::new(workspace)
        }
        _ => false,
    }
}
pub fn shell_join(words: &[&str]) -> String {
    words
        .iter()
        .map(|s| {
            if !s.is_empty()
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c))
            {
                s.to_string()
            } else {
                format!("'{}'", s.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
