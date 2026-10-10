//! Windows: a client command found where it is installed and run to its end within a time,
//! with its output kept and everything it started held in a job.
use super::{OwnedGroup, interrupted, windows::end_with_this_process};
use crate::error::Error;
use std::{
    env, fs,
    io::{self, Read},
    os::windows::process::CommandExt,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

/// The command for a client by its name, from the directories of `PATH` in their order. An
/// `.exe` is started itself. npm installs a `.cmd` that starts `node` with a script of the
/// package, and a `.cmd` cannot be given an argument with a line break in it, as a notice is.
/// So the `.cmd` is read and not run: `node` is started with the script it names.
fn installed(program: &str) -> io::Result<Command> {
    let missing = || io::Error::from(io::ErrorKind::NotFound);
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path).filter(|d| !d.as_os_str().is_empty()) {
        let executable = directory.join(format!("{program}.exe"));
        if executable.is_file() {
            return Ok(Command::new(executable));
        }
        let Ok(shim) = fs::read_to_string(directory.join(format!("{program}.cmd"))) else {
            continue;
        };
        // The script is quoted in the shim, after the directory of the shim itself.
        let script = shim
            .split('"')
            .filter_map(|quoted| {
                quoted
                    .strip_prefix(r"%dp0%\")
                    .or_else(|| quoted.strip_prefix(r"%~dp0\"))
            })
            .find(|script| script.ends_with("js"))
            .map(|script| directory.join(script))
            .filter(|script| script.is_file())
            .ok_or_else(missing)?;
        // As the shim chooses: the `node` beside it, then the one on the path.
        let node = std::iter::once(directory.clone())
            .chain(env::split_paths(&path))
            .map(|d| d.join("node.exe"))
            .find(|node| node.is_file())
            .ok_or_else(missing)?;
        let mut command = Command::new(node);
        command.arg(script);
        return Ok(command);
    }
    Err(missing())
}

/// Reads a pipe to its end on a thread of its own, so that a full pipe never holds the wait.
fn kept(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut data = Vec::new();
        let _ = send.send(pipe.read_to_end(&mut data).map(|_| data));
    });
    receive
}

/// Capture a bounded-lived child. What it started is in its job and ends with it.
pub fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<Output, Error> {
    end_with_this_process();
    let mut child = installed(program)?
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    let group = match OwnedGroup::new(child.id()) {
        Ok(group) => group,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.into());
        }
    };
    let stdout = kept(child.stdout.take().ok_or(Error::System("os_error"))?);
    let stderr = kept(child.stderr.take().ok_or(Error::System("os_error"))?);
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (None, None);
    loop {
        out = out.or_else(|| stdout.try_recv().ok());
        err = err.or_else(|| stderr.try_recv().ok());
        let exited = match group.exited() {
            Ok(exited) => exited,
            Err(e) => {
                let _ = group.end();
                return Err(e.into());
            }
        };
        if exited && out.is_some() && err.is_some() {
            group.end()?;
            return Ok(Output {
                status: child.wait()?,
                stdout: out.unwrap_or_else(|| Ok(Vec::new()))?,
                stderr: err.unwrap_or_else(|| Ok(Vec::new()))?,
            });
        }
        if interrupted() || Instant::now() >= deadline {
            group.end()?;
            child.wait()?;
            return Err(if interrupted() {
                Error::Interrupted
            } else {
                Error::System("command_timed_out")
            });
        }
        thread::sleep(Duration::from_millis(5));
    }
}
