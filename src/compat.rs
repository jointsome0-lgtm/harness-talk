//! What is left of the Python program this one replaced. Its exception class names are saved
//! as notification details and shown by discovery, and a few helpers repeat how Python wrote
//! or compared a value. All of it is gathered here so that it can be removed in one place
//! (issue #76). Nothing here is to be polished.
use crate::{
    error::{Error, Failure},
    os::errno,
};
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
};

pub(crate) const ATTRIBUTE_ERROR: Failure = Failure::Class("AttributeError");
pub(crate) const CALLED_PROCESS_ERROR: Failure = Failure::Class("CalledProcessError");
pub(crate) const FILE_NOT_FOUND_ERROR: Failure = Failure::Class("FileNotFoundError");
pub(crate) const JSON_DECODE_ERROR: Failure = Failure::Class("JSONDecodeError");
pub(crate) const KEY_ERROR: Failure = Failure::Class("KeyError");
pub(crate) const KEYBOARD_INTERRUPT: Failure = Failure::Class("KeyboardInterrupt");
pub(crate) const OPERATIONAL_ERROR: Failure = Failure::Class("OperationalError");
pub(crate) const OS_ERROR: Failure = Failure::Class("OSError");
pub(crate) const PERMISSION_ERROR: Failure = Failure::Class("PermissionError");
pub(crate) const SSL_ERROR: Failure = Failure::Class("SSLError");
pub(crate) const TIMEOUT_ERROR: Failure = Failure::Class("TimeoutError");
pub(crate) const TIMEOUT_EXPIRED: Failure = Failure::Class("TimeoutExpired");
pub(crate) const TOML_DECODE_ERROR: Failure = Failure::Class("TOMLDecodeError");
pub(crate) const TYPE_ERROR: Failure = Failure::Class("TypeError");
pub(crate) const UNICODE_DECODE_ERROR: Failure = Failure::Class("UnicodeDecodeError");
pub(crate) const UNICODE_ENCODE_ERROR: Failure = Failure::Class("UnicodeEncodeError");
pub(crate) const VALUE_ERROR: Failure = Failure::Class("ValueError");

/// `http.client` exception names. The OpenCode adapter reports them as `opencode_<name>`.
pub(crate) mod http {
    pub(crate) const BAD_STATUS_LINE: &str = "BadStatusLine";
    pub(crate) const HTTP_EXCEPTION: &str = "HTTPException";
    pub(crate) const INCOMPLETE_READ: &str = "IncompleteRead";
    pub(crate) const INVALID_URL: &str = "InvalidURL";
    pub(crate) const LINE_TOO_LONG: &str = "LineTooLong";
    pub(crate) const REMOTE_DISCONNECTED: &str = "RemoteDisconnected";
    pub(crate) const UNKNOWN_PROTOCOL: &str = "UnknownProtocol";
}

impl Failure {
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::Coded(_) => "ValueError",
            Self::Class(name) => name,
        }
    }
}
impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        if let Some(errno) = e.raw_os_error() {
            return Self::Class(match errno {
                errno::ENOENT => "FileNotFoundError",
                errno::EACCES | errno::EPERM => "PermissionError",
                errno::EISDIR => "IsADirectoryError",
                errno::ENOTDIR => "NotADirectoryError",
                errno::EEXIST => "FileExistsError",
                errno::EINTR => "InterruptedError",
                errno::EAGAIN | errno::EALREADY | errno::EINPROGRESS => "BlockingIOError",
                errno::EPIPE | errno::ESHUTDOWN => "BrokenPipeError",
                errno::ECONNABORTED => "ConnectionAbortedError",
                errno::ECONNREFUSED => "ConnectionRefusedError",
                errno::ECONNRESET => "ConnectionResetError",
                errno::ETIMEDOUT => "TimeoutError",
                errno::ECHILD => "ChildProcessError",
                errno::ESRCH => "ProcessLookupError",
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

/// The class Python raised for one bad record, where an OSError ended the whole source.
pub(crate) fn one_record(failure: &Failure) -> bool {
    matches!(
        failure,
        Failure::Class(
            "ValueError"
                | "JSONDecodeError"
                | "UnicodeDecodeError"
                | "KeyError"
                | "TypeError"
                | "AttributeError"
        )
    )
}

/// An htalk error as an adapter reports it: its code, or only the class name.
pub(crate) fn failure(e: Error) -> Failure {
    match e {
        Error::Code(c) => Failure::Coded(c),
        Error::Value(_) => VALUE_ERROR,
        Error::Io(e) => e.into(),
        Error::Db(e) => e.into(),
        Error::Interrupted => KEYBOARD_INTERRUPT,
    }
}

/// The recorded detail of a failure that is not a typed outcome: htalk's own
/// validation errors are plain ValueErrors, others keep only their class name.
pub(crate) fn failure_detail(e: Error) -> String {
    match e {
        Error::Code(_) | Error::Value(_) => VALUE_ERROR.to_string(),
        other => failure(other).to_string(),
    }
}

pub(crate) fn python_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// Python-style `value[key]`: KeyError for a missing key, TypeError for a non-object.
pub(crate) fn index<'a>(value: &'a Value, key: &str) -> Result<&'a Value, Failure> {
    match value {
        Value::Object(map) => map.get(key).ok_or(Failure::Class("KeyError")),
        _ => Err(Failure::Class("TypeError")),
    }
}

/// Decode captured output as Python's `text=True` does, with universal newlines.
pub(crate) fn text_output(output: &std::process::Output) -> Result<String, Failure> {
    let decode = |bytes: &[u8]| {
        std::str::from_utf8(bytes)
            .map(|s| s.replace("\r\n", "\n").replace('\r', "\n"))
            .map_err(|_| Failure::Class("UnicodeDecodeError"))
    };
    let stdout = decode(&output.stdout)?;
    decode(&output.stderr)?;
    Ok(stdout)
}

/// Canonical UUID text as Python's `str(uuid.UUID(value))`, or None when Python raises ValueError.
pub(crate) fn python_uuid(value: &str) -> Option<String> {
    crate::validate::uuid(value).ok()
}

/// The string form of Python's `Path(value)`: repeated separators and `.` parts collapse.
pub(crate) fn python_path(value: &str) -> String {
    let root = if value.starts_with("//") && !value.starts_with("///") {
        "//"
    } else if value.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = value
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let joined = format!("{root}{}", parts.join("/"));
    if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

/// Python's OSError subclass for an errno, recorded by class name only.
pub(crate) fn io_failure(error: io::Error) -> Failure {
    Failure::from(error)
}

/// An I/O failure on a socket with a timeout: an expired timeout is Python's TimeoutError.
pub(crate) fn socket_failure(error: io::Error) -> Failure {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Failure::Class("TimeoutError"),
        _ => io_failure(error),
    }
}

/// `json.dumps` with its default separators and ASCII escaping, so client frames match 0.4.0.
pub(crate) fn python_dumps(value: &Value) -> String {
    fn string(out: &mut String, text: &str) {
        out.push('"');
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                ' '..='~' => out.push(c),
                _ => {
                    for unit in c.encode_utf16(&mut [0; 2]) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
        }
        out.push('"');
    }
    fn write(out: &mut String, value: &Value) {
        match value {
            Value::String(text) => string(out, text),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(out, item);
                }
                out.push(']');
            }
            Value::Object(map) => {
                out.push('{');
                for (i, (key, item)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    string(out, key);
                    out.push_str(": ");
                    write(out, item);
                }
                out.push('}');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(&mut out, value);
    out
}

/// The recipient socket must be a Unix socket owned by this account.
pub(crate) fn owned_socket(path: &Path) -> Result<PathBuf, Failure> {
    crate::os::owned_socket(Path::new(&python_path(&path.to_string_lossy())))
}
