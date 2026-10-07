//! One child run to its end with its output kept, inside a process group this program owns.
use super::group::OwnedGroup;
use crate::{
    compat,
    error::Failure,
    os::{Grouped, interrupted, set_nonblocking},
};
use std::{
    io::{self, Read},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Capture a bounded-lived child without blocking on full stdout/stderr pipes.
pub fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, Failure> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .own_group()
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
        group: &mut OwnedGroup,
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
    let mut group = match OwnedGroup::new(child.id()) {
        Ok(group) => group,
        Err(e) => {
            // The retained, unreaped Child still owns this direct process.
            // Group ownership has not been established, so signal no group.
            let _ = child.kill();
            reap_direct(&mut child);
            return Err(e.into());
        }
    };
    let mut stdout = child.stdout.take().ok_or(compat::OS_ERROR)?;
    let mut stderr = child.stderr.take().ok_or(compat::OS_ERROR)?;
    // Nonblocking reads keep the deadline in force even if a grandchild
    // inherits an output pipe after the direct child exits.
    if let Err(error) = set_nonblocking(&stdout).and_then(|()| set_nonblocking(&stderr)) {
        finish(&mut group, &mut child)?;
        return Err(error.into());
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
            return Err(if interrupted() {
                compat::KEYBOARD_INTERRUPT
            } else {
                compat::TIMEOUT_EXPIRED
            });
        }
        thread::sleep(Duration::from_millis(5));
    }
}
