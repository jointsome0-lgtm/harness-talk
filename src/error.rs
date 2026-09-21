use std::{fmt, io};

/// Only fixed codes and class names cross a notification or discovery boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Coded(String),
    Class(&'static str),
}
impl Failure {
    pub fn coded(code: impl Into<String>) -> Self {
        Self::Coded(code.into())
    }
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::Coded(_) => "ValueError",
            Self::Class(name) => name,
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Coded(code) => f.write_str(code),
            Self::Class(name) => f.write_str(name),
        }
    }
}
impl std::error::Error for Failure {}
impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        if let Some(errno) = e.raw_os_error() {
            return Self::Class(match errno {
                libc::ENOENT => "FileNotFoundError",
                libc::EACCES | libc::EPERM => "PermissionError",
                libc::EISDIR => "IsADirectoryError",
                libc::ENOTDIR => "NotADirectoryError",
                libc::EEXIST => "FileExistsError",
                libc::EINTR => "InterruptedError",
                libc::EAGAIN | libc::EALREADY | libc::EINPROGRESS => "BlockingIOError",
                libc::EPIPE | libc::ESHUTDOWN => "BrokenPipeError",
                libc::ECONNABORTED => "ConnectionAbortedError",
                libc::ECONNREFUSED => "ConnectionRefusedError",
                libc::ECONNRESET => "ConnectionResetError",
                libc::ETIMEDOUT => "TimeoutError",
                libc::ECHILD => "ChildProcessError",
                libc::ESRCH => "ProcessLookupError",
                _ => "OSError",
            });
        }
        use io::ErrorKind::*;
        Self::Class(match e.kind() {
            NotFound => "FileNotFoundError",
            PermissionDenied => "PermissionError",
            ConnectionRefused => "ConnectionRefusedError",
            ConnectionReset => "ConnectionResetError",
            ConnectionAborted => "ConnectionAbortedError",
            BrokenPipe => "BrokenPipeError",
            TimedOut | WouldBlock => "TimeoutError",
            _ => "OSError",
        })
    }
}
impl From<serde_json::Error> for Failure {
    fn from(_: serde_json::Error) -> Self {
        Self::Class("JSONDecodeError")
    }
}
impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Self {
        Self::Class(match e {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                ) =>
            {
                "DatabaseError"
            }
            _ => "OperationalError",
        })
    }
}

#[derive(Debug)]
pub enum Error {
    Code(String),
    Value(String),
    Io(io::Error),
    Db(rusqlite::Error),
    Interrupted,
}
impl Error {
    pub fn code(code: impl Into<String>) -> Self {
        Self::Code(code.into())
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(s) | Self::Value(s) => f.write_str(s),
            Self::Io(e) => e.fmt(f),
            Self::Db(e) => e.fmt(f),
            Self::Interrupted => f.write_str("interrupted"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}
impl From<Failure> for Error {
    fn from(e: Failure) -> Self {
        Self::Code(e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::code("invalid_json")
    }
}
