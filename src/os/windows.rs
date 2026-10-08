//! Windows: where the mailbox is, how its files are opened and locked, Ctrl-C and Ctrl-Break,
//! a child with everything it starts, and an output whose reader has gone.
use super::home;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_PIPE_CLOSING_STATE, FILE_PIPE_LOCAL_INFORMATION, FilePipeLocalInformation,
    NtQueryInformationFile,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType, LOCKFILE_EXCLUSIVE_LOCK, LockFileEx},
    System::{
        Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
        },
        IO::IO_STATUS_BLOCK,
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NEW_PROCESS_GROUP, GetCurrentProcess, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
            WaitForSingleObject,
        },
    },
};

pub const DB_HELP: &str = "Shared SQLite file. Default: HTALK_DB, then $XDG_DATA_HOME/harness-talk/mail.sqlite3, then %LOCALAPPDATA%\\harness-talk\\mail.sqlite3. Only peer add creates a missing file.";

/// The mailbox when `--db` names none, as `DB_HELP` says.
pub fn default_db() -> PathBuf {
    if let Some(db) = env::var_os("HTALK_DB").filter(|v| !v.is_empty()) {
        return db.into();
    }
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(data)
            .join("harness-talk")
            .join("mail.sqlite3");
    }
    env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join("AppData").join("Local"), PathBuf::from)
        .join("harness-talk")
        .join("mail.sqlite3")
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn note_interrupt(event: u32) -> i32 {
    let ours = event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT;
    if ours {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
    ours.into()
}
/// Ctrl-C and Ctrl-Break are an interrupt. A child that leads its own group hears the second.
pub fn install_interrupt_handler() -> io::Result<()> {
    if unsafe { SetConsoleCtrlHandler(Some(note_interrupt), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// The request to terminate: Ctrl-Break, or the console closing.
pub struct Terminate(
    tokio::signal::windows::CtrlBreak,
    tokio::signal::windows::CtrlClose,
);
impl Terminate {
    pub async fn recv(&mut self) -> Option<()> {
        tokio::select! {
            asked = self.0.recv() => asked,
            asked = self.1.recv() => asked,
        }
    }
}
/// The two requests to stop, as streams for an async runtime: Ctrl-C as an interrupt, then a
/// request to terminate.
pub fn stop_requests() -> io::Result<(tokio::signal::windows::CtrlC, Terminate)> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};
    Ok((ctrl_c()?, Terminate(ctrl_break()?, ctrl_close()?)))
}

/// A child that leads a process group of its own, so that Ctrl-Break can be sent to it alone.
pub trait Grouped {
    fn own_group(&mut self) -> &mut Self;
}
impl Grouped for tokio::process::Command {
    fn own_group(&mut self) -> &mut Self {
        end_with_this_process();
        self.creation_flags(CREATE_NEW_PROCESS_GROUP)
    }
}

/// A handle this process owns, closed on drop.
struct Owned(HANDLE);
// SAFETY: a kernel handle may be used from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A job that ends its members when its last handle is closed, as when its holder is killed.
fn closing_job() -> io::Result<Owned> {
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = Owned(job);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

/// Puts this process in such a job, once, before it starts a child. The child is born in it,
/// so what the child starts before it has entered a job of its own still ends when this process
/// does. Where this process can enter no job, that is not so.
fn end_with_this_process() {
    static JOB: OnceLock<Option<Owned>> = OnceLock::new();
    JOB.get_or_init(|| {
        let job = closing_job().ok()?;
        (unsafe { AssignProcessToJobObject(job.0, GetCurrentProcess()) } != 0).then_some(job)
    });
}

/// A child and everything it starts, held in a job. The caller retains the direct Child, so
/// the number names this child. What the child started before it entered the job is not in it
/// and ends with this process.
pub struct OwnedGroup {
    leader: u32,
    process: Owned,
    job: Owned,
}
impl OwnedGroup {
    pub fn new(pid: u32) -> io::Result<Self> {
        const SYNCHRONIZE: u32 = 0x0010_0000;
        let rights =
            SYNCHRONIZE | PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION;
        let process = unsafe { OpenProcess(rights, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        // A server that is killed runs no cleanup. Its handle to the job closes with it, and
        // that is the end of the members.
        let group = Self {
            leader: pid,
            process: Owned(process),
            job: closing_job()?,
        };
        if unsafe { AssignProcessToJobObject(group.job.0, group.process.0) } == 0 {
            let refused = io::Error::last_os_error();
            // A child that has exited enters no job, and has nothing left to stop.
            if !group.exited()? {
                return Err(refused);
            }
        }
        Ok(group)
    }

    /// Whether the child has exited.
    pub fn exited(&self) -> io::Result<bool> {
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            0x102 => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Ctrl-Break to the child's group. Without a console in common nothing is delivered.
    pub(crate) fn interrupt(&self) -> io::Result<()> {
        if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.leader) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn active(&self) -> io::Result<u32> {
        let mut found = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let size = size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32;
        let class = JobObjectBasicAccountingInformation;
        let into = (&raw mut found).cast();
        if unsafe { QueryInformationJobObject(self.job.0, class, into, size, std::ptr::null_mut()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(found.ActiveProcesses)
    }

    /// Asks a child that still runs to stop, and two seconds later ends what runs in the job.
    /// Ctrl-Break is the one request Windows has, and it goes to the group that a living child
    /// leads. A child that is gone leaves nothing to ask, and where the request cannot be sent
    /// there is nothing to wait for: the job is ended at once.
    pub async fn finish_async(&mut self) -> io::Result<()> {
        let start = Instant::now();
        let asked = !self.exited()? && self.interrupt().is_ok();
        while self.active()? != 0 {
            if !asked || start.elapsed() >= Duration::from_secs(2) {
                unsafe { TerminateJobObject(self.job.0, 1) };
            }
            if start.elapsed() >= Duration::from_secs(4) {
                return Err(io::Error::other(
                    "Owned process cleanup did not complete in four seconds",
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
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

/// Something to lock for a directory: Windows locks no directory, so it is a file inside it.
pub fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.join("lock"))
}
/// Waits for the exclusive lock. It is released when the file is closed.
pub fn lock(file: &File) -> io::Result<()> {
    let mut whole = unsafe { std::mem::zeroed() };
    let handle = file.as_raw_handle();
    if unsafe { LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, &mut whole) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
/// The other end of `io` is gone, seen without reading or writing. A pipe says so when it is
/// asked for its state. What is no pipe, and a pipe that may not be asked, say nothing, and a
/// closed output is then noticed at the next write.
pub fn hung_up(io: &impl AsRawHandle) -> bool {
    let handle = io.as_raw_handle();
    if unsafe { GetFileType(handle) } != FILE_TYPE_PIPE {
        return false;
    }
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut pipe: FILE_PIPE_LOCAL_INFORMATION = unsafe { std::mem::zeroed() };
    let answered = unsafe {
        NtQueryInformationFile(
            handle,
            &mut status,
            (&raw mut pipe).cast(),
            size_of::<FILE_PIPE_LOCAL_INFORMATION>() as u32,
            FilePipeLocalInformation,
        )
    };
    answered >= 0 && pipe.NamedPipeState == FILE_PIPE_CLOSING_STATE
}
