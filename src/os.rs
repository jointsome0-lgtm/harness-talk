use crate::error::Failure;
use std::{env, fs, io, path::{Component, Path, PathBuf}, sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, SystemTime, UNIX_EPOCH}};
use std::os::unix::fs::{FileTypeExt, MetadataExt};

static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
pub fn install_interrupt_handler() -> io::Result<()> {
    let flag = INTERRUPTED.get_or_init(|| Arc::new(AtomicBool::new(false))).clone();
    signal_hook::flag::register(signal_hook::consts::SIGINT, flag)?;
    Ok(())
}
pub fn interrupted() -> bool { INTERRUPTED.get().is_some_and(|f| f.load(Ordering::Relaxed)) }
pub fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() }
pub fn home() -> PathBuf { env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")) }
pub fn expand_user(path: &Path) -> PathBuf {
    if path == Path::new("~") { home() }
    else if let Ok(rest) = path.strip_prefix("~/") { home().join(rest) }
    else { path.to_path_buf() }
}
pub fn resolve(path: &Path) -> PathBuf {
    fn walk(path: &Path, depth: usize) -> PathBuf {
        let mut out = if path.is_absolute() { PathBuf::new() } else { env::current_dir().unwrap_or_else(|_| PathBuf::from("/")) };
        for part in path.components() {
            match part {
                Component::RootDir => out = PathBuf::from("/"),
                Component::CurDir => (),
                Component::ParentDir => { out.pop(); },
                Component::Normal(p) => {
                    out.push(p);
                    if depth < 40 && let Ok(target) = fs::read_link(&out) {
                        let target = if target.is_absolute() { target } else { out.parent().unwrap_or(Path::new("/")).join(target) };
                        out = walk(&target, depth + 1);
                    }
                },
                Component::Prefix(_) => (),
            }
        }
        out
    }
    walk(&expand_user(path), 0)
}
pub fn resolve_strict(path: &Path) -> io::Result<PathBuf> { fs::canonicalize(expand_user(path)) }
pub fn same_workspace(native: Option<&str>, registered: &str) -> bool {
    native.and_then(|p| resolve_strict(Path::new(p)).ok())
        .zip(resolve_strict(Path::new(registered)).ok()).is_some_and(|(a,b)| a == b)
}
pub fn owned_socket(path: &Path) -> Result<PathBuf, Failure> {
    let metadata = fs::metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(Failure::coded("recipient_socket_unavailable"));
    }
    Ok(path.to_path_buf())
}
pub fn shell_join(words: &[&str]) -> String {
    words.iter().map(|s| {
        if !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c)) { s.to_string() }
        else { format!("'{}'", s.replace('\'', "'\"'\"'")) }
    }).collect::<Vec<_>>().join(" ")
}

/// Capture a bounded-lived child without blocking on full stdout/stderr pipes.
pub fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<std::process::Output, Failure> {
    use std::{io::Read, process::{Command, Stdio}, thread, time::Instant};
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn()?;
    let stdout = child.stdout.take().ok_or(Failure::Class("OSError"))?;
    let stderr = child.stderr.take().ok_or(Failure::Class("OSError"))?;
    let read = |mut pipe: Box<dyn Read + Send>| thread::spawn(move || { let mut data=Vec::new(); pipe.read_to_end(&mut data).map(|_| data) });
    let out = read(Box::new(stdout)); let err = read(Box::new(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? { break Ok(status); }
        if interrupted() || Instant::now() >= deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
            let _ = child.wait();
            break Err(Failure::Class(if interrupted() { "KeyboardInterrupt" } else { "TimeoutExpired" }));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = out.join().map_err(|_| Failure::Class("OSError"))??;
    let stderr = err.join().map_err(|_| Failure::Class("OSError"))??;
    Ok(std::process::Output { status: status?, stdout, stderr })
}

