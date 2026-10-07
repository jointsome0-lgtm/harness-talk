use std::{borrow::Cow, fmt, io};

/// Only fixed codes cross a notification or discovery boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// What htalk or a client refused, or what a client gave that could not be read.
    Coded(Cow<'static, str>),
    /// What the system under them could not do: a file, a socket, a process, a database.
    System(&'static str),
}
impl Failure {
    /// A client's answer or file that is not what its protocol says.
    pub const INVALID_DATA: Self = Self::Coded(Cow::Borrowed("invalid_client_data"));
    pub const INVALID_UTF8: Self = Self::Coded(Cow::Borrowed("invalid_utf8"));
    pub const FILE_NOT_FOUND: Self = Self::System("file_not_found");
    pub const PERMISSION_DENIED: Self = Self::System("permission_denied");
    pub const OS_ERROR: Self = Self::System("os_error");
    pub const INTERRUPTED: Self = Self::System("interrupted");
    pub const COMMAND_FAILED: Self = Self::System("command_failed");
    pub const COMMAND_TIMED_OUT: Self = Self::System("command_timed_out");

    pub fn coded(code: impl Into<Cow<'static, str>>) -> Self {
        Self::Coded(code.into())
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Coded(code) => code,
            Self::System(code) => code,
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Failure {}
/// An operating system failure.
impl From<io::ErrorKind> for Failure {
    fn from(kind: io::ErrorKind) -> Self {
        use io::ErrorKind::*;
        Self::System(match kind {
            NotFound => "file_not_found",
            PermissionDenied => "permission_denied",
            ConnectionRefused => "connection_refused",
            ConnectionReset | ConnectionAborted | BrokenPipe => "connection_closed",
            TimedOut | WouldBlock => "timed_out",
            _ => "os_error",
        })
    }
}
impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        e.kind().into()
    }
}
impl From<serde_json::Error> for Failure {
    fn from(_: serde_json::Error) -> Self {
        Self::coded("invalid_json")
    }
}
impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                ) =>
            {
                Self::System("client_database_corrupt")
            }
            _ => Self::System("client_database_unavailable"),
        }
    }
}
/// An htalk error as an adapter reports it.
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        match e {
            Error::Code(code) => Self::coded(code),
            Error::Io(e) => e.into(),
            Error::Db(e) => e.into(),
            Error::Interrupted => Self::INTERRUPTED,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Code(String),
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
            Self::Code(s) => f.write_str(s),
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
