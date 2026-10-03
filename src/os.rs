use crate::error::Failure;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
pub fn install_interrupt_handler() -> io::Result<()> {
    let flag = INTERRUPTED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    signal_hook::flag::register(signal_hook::consts::SIGINT, flag)?;
    Ok(())
}
pub fn interrupted() -> bool {
    INTERRUPTED.get().is_some_and(|f| f.load(Ordering::Relaxed))
}
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
                Component::RootDir => out = PathBuf::from("/"),
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
                Component::Prefix(_) => (),
            }
        }
        out
    }
    walk(&expand_user(path), 0)
}
pub fn resolve_strict(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(expand_user(path))
}
pub fn same_workspace(native: Option<&str>, registered: &str) -> bool {
    native
        .and_then(|p| resolve_strict(Path::new(p)).ok())
        .zip(resolve_strict(Path::new(registered)).ok())
        .is_some_and(|(a, b)| a == b)
}
pub fn owned_socket(path: &Path) -> Result<PathBuf, Failure> {
    let metadata = fs::metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::getuid() } {
        return Err(Failure::coded("recipient_socket_unavailable"));
    }
    Ok(path.to_path_buf())
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

/// Capture a bounded-lived child without blocking on full stdout/stderr pipes.
pub fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, Failure> {
    run_command_with_group(
        program,
        args,
        timeout,
        crate::process_cleanup::OwnedGroup::new,
    )
}

fn run_command_with_group(
    program: &str,
    args: &[&str],
    timeout: Duration,
    make_group: impl FnOnce(u32) -> io::Result<crate::process_cleanup::OwnedGroup>,
) -> Result<std::process::Output, Failure> {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::{
        io::Read,
        process::{Command, Stdio},
        thread,
        time::Instant,
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    // Keep direct-child reaping bounded even if termination cannot complete.
    fn reap_direct(child: &mut std::process::Child) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => thread::sleep(Duration::from_millis(5)),
            }
        }
    }
    fn finish(
        group: &mut crate::process_cleanup::OwnedGroup,
        child: &mut std::process::Child,
    ) -> io::Result<std::process::ExitStatus> {
        let cleanup = group.finish();
        if cleanup.is_err() {
            let _ = group.kill_leader();
            reap_direct(child);
            cleanup?;
        }
        child.wait()
    }
    let mut group = match make_group(child.id()) {
        Ok(group) => group,
        Err(e) => {
            // The retained, unreaped Child still owns this direct process.
            // Group ownership has not been established, so signal no group.
            let _ = child.kill();
            reap_direct(&mut child);
            return Err(e.into());
        }
    };
    let mut stdout = child.stdout.take().ok_or(Failure::Class("OSError"))?;
    let mut stderr = child.stderr.take().ok_or(Failure::Class("OSError"))?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        // Nonblocking reads keep the deadline in force even if a grandchild
        // inherits an output pipe after the direct child exits.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let error = io::Error::last_os_error();
            finish(&mut group, &mut child)?;
            return Err(error.into());
        }
    }
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let read = |pipe: &mut dyn Read, data: &mut Vec<u8>, done: &mut bool| -> io::Result<()> {
        if *done {
            return Ok(());
        }
        let mut buffer = [0; 65536];
        match pipe.read(&mut buffer) {
            Ok(0) => *done = true,
            Ok(n) => data.extend_from_slice(&buffer[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
        Ok(())
    };
    loop {
        let result = (|| -> io::Result<()> {
            read(&mut stdout, &mut out, &mut out_done)?;
            read(&mut stderr, &mut err, &mut err_done)?;
            Ok(())
        })();
        if let Err(e) = result {
            finish(&mut group, &mut child)?;
            return Err(e.into());
        }
        let exited = match group.exited() {
            Ok(exited) => exited,
            Err(e) => {
                let _ = group.kill_leader();
                reap_direct(&mut child);
                return Err(e.into());
            }
        };
        if exited && out_done && err_done {
            let status = finish(&mut group, &mut child)?;
            return Ok(std::process::Output {
                status,
                stdout: out,
                stderr: err,
            });
        }
        if interrupted() || Instant::now() >= deadline {
            finish(&mut group, &mut child)?;
            return Err(Failure::Class(if interrupted() {
                "KeyboardInterrupt"
            } else {
                "TimeoutExpired"
            }));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/unit/os_process_cleanup.rs"]
mod process_cleanup_tests;
