use std::{borrow::Cow, fmt, io};

/// Why a command or a call to a client failed. It is written as its fixed code, and only that
/// crosses a notification or discovery boundary.
#[derive(Debug)]
pub enum Error {
    /// What htalk or a client refused, or what a client gave that could not be read.
    Code(Cow<'static, str>),
    /// What the system under a client could not do, where no `io::Error` says it.
    System(&'static str),
    Io(io::Error),
    /// The mailbox's own database.
    Db(rusqlite::Error),
    Interrupted,
}
impl Error {
    pub fn code(code: impl Into<Cow<'static, str>>) -> Self {
        Self::Code(code.into())
    }

    /// A client's answer or file that is not what its protocol says.
    pub fn invalid_data() -> Self {
        Self::code("invalid_client_data")
    }

    /// What a part answers on a system it is not ported to. `build.rs` says which those are.
    pub fn not_ported() -> Self {
        Self::code("unsupported_on_this_platform")
    }

    pub fn invalid_utf8() -> Self {
        Self::code("invalid_utf8")
    }

    /// The same error where the database read was a client's and not the mailbox.
    pub fn client(self) -> Self {
        match self.fixed() {
            "database_corrupt" => Self::System("client_database_corrupt"),
            "database_unavailable" => Self::System("client_database_unavailable"),
            _ => self,
        }
    }

    pub fn fixed(&self) -> &str {
        use rusqlite::ErrorCode::*;
        match self {
            Self::Code(code) => code,
            Self::System(code) => code,
            Self::Io(e) => io_code(e.kind()),
            Self::Db(e) => match e.sqlite_error_code() {
                Some(DatabaseCorrupt | NotADatabase) => "database_corrupt",
                _ => "database_unavailable",
            },
            Self::Interrupted => "interrupted",
        }
    }

    /// The system's own words for a file or database error, as a mailbox command answers.
    pub fn text(&self) -> String {
        match self {
            Self::Io(e) => e.to_string(),
            Self::Db(e) => e.to_string(),
            _ => self.fixed().to_owned(),
        }
    }

    /// Whether the system failed. In discovery that ends a source; a refusal rejects one record.
    pub fn system(&self) -> bool {
        !matches!(self, Self::Code(_))
    }
}
/// The fixed code of an operating system failure.
pub fn io_code(kind: io::ErrorKind) -> &'static str {
    use io::ErrorKind::*;
    match kind {
        NotFound => "file_not_found",
        PermissionDenied => "permission_denied",
        ConnectionRefused => "connection_refused",
        ConnectionReset | ConnectionAborted | BrokenPipe => "connection_closed",
        TimedOut | WouldBlock => "timed_out",
        _ => "os_error",
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.fixed())
    }
}
impl std::error::Error for Error {}
/// Two errors are the same when their codes are.
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.fixed() == other.fixed()
    }
}
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
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::code("invalid_json")
    }
}
