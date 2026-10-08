//! Windows: where the mailbox is, how its files are opened and locked, Ctrl-C and Ctrl-Break,
//! and a child with everything it starts. A closed output is not ported yet and says below
//! what happens until then.
use super::home;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx},
    System::{
        Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
        },
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, OpenProcess, OpenThread,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE, ResumeThread,
            THREAD_SUSPEND_RESUME, WaitForSingleObject,
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
/// It is created suspended and does not run until `OwnedGroup::new` has put it in a job, so
/// nothing it starts is outside the job. Until then it ends with the handle the caller holds.
pub trait Grouped {
    fn own_group(&mut self) -> &mut Self;
}
impl Grouped for tokio::process::Command {
    fn own_group(&mut self) -> &mut Self {
        self.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED)
            .kill_on_drop(true)
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

/// A child and everything it starts, held in a job. The caller retains the direct Child, so
/// the number names this child.
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
        let process = Owned(process);
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let group = Self {
            leader: pid,
            process,
            job: Owned(job),
        };
        // A server that is killed runs no cleanup. Its handle to the job closes with it, and
        // this makes that the end of the members.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                group.job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { AssignProcessToJobObject(group.job.0, group.process.0) } == 0 {
            let refused = io::Error::last_os_error();
            // A child that has exited enters no job, and has nothing left to stop.
            if !group.exited()? {
                return Err(refused);
            }
        }
        group.resume()?;
        Ok(group)
    }

    /// Lets the child run. It was created suspended and has the one thread it was created
    /// with; the list of every thread of the system is where Windows names it.
    fn resume(&self) -> io::Result<()> {
        let threads = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if threads == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let threads = Owned(threads);
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        let mut more = unsafe { Thread32First(threads.0, &mut entry) };
        while more != 0 {
            if entry.th32OwnerProcessID == self.leader {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = Owned(thread);
                if unsafe { ResumeThread(thread.0) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                resumed = true;
            }
            more = unsafe { Thread32Next(threads.0, &mut entry) };
        }
        // A child that was let run by no one would wait for ever.
        if resumed || self.exited()? {
            Ok(())
        } else {
            Err(io::Error::other("The suspended child has no thread"))
        }
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
    /// leads. A child that is gone or a console that is not shared leaves nothing to ask, and
    /// the job is ended at once.
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
/// Not ported: a closed output is noticed at the next write.
pub fn hung_up<T>(_io: &T) -> bool {
    false
}
