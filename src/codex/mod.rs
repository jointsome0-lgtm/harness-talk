//! Codex notification, queue-receipt cleanup and probing.
//!
//! Only fixed htalk codes and exception-class names leave this module as details, so foreign
//! error text, paths and credentials never reach the shared database or command output.
pub mod rpc;
pub mod state;

use crate::model::NativePeer as Peer;
use crate::{error::Failure, model::*, os};
use rpc::Rpc;
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

const QUEUE_TIMEOUT: Duration = Duration::from_secs(20);

pub fn notify(peer: &Peer, message_id: &str, body: &str, skip: Skip<'_>) -> Outcome {
    match socket(peer) {
        None => notify_cli(peer, body, skip),
        Some(path) => notify_socket(peer, path, message_id, body, skip),
    }
}

/// Delete only this acknowledged message's saved Codex queue receipt.
pub fn dismiss(peer: &Peer, message: &Message) -> Cleanup {
    let row = &message.row;
    if row.ack_at.is_none() || peer.name != row.recipient {
        return Cleanup::new(CleanupStatus::Skipped)
            .detail("message_not_acknowledged_by_recipient");
    }
    if peer.harness != Harness::Codex {
        return Cleanup::new(CleanupStatus::Unsupported)
            .detail("client_has_no_notification_removal");
    }
    let socket = socket(peer);
    let prefix = if socket.is_some() {
        "codex_queued:"
    } else {
        "codex_cli_queued:"
    };
    let detail = row.notification_detail.as_deref().unwrap_or("");
    if row.notification_started_at.is_some() && row.notification_finished_at.is_none() {
        return Cleanup::new(CleanupStatus::Pending)
            .detail("notification_submission_has_no_completion_receipt");
    }
    let Some(queued) = detail
        .strip_prefix(prefix)
        .filter(|_| row.submission == Submission::Submitted)
    else {
        return Cleanup::new(CleanupStatus::Skipped).detail("no_confirmed_queue_receipt");
    };
    let mut attempted = false;
    let result = (|| -> Result<(bool, String), Failure> {
        let queue_id = python_uuid(queued).ok_or(Failure::Class("ValueError"))?;
        let mut rpc = match socket {
            Some(path) => {
                let mut rpc = Rpc::connect_unix(path)?;
                check_live(&mut rpc, peer)?;
                rpc
            }
            None => {
                saved_identity(peer)?;
                Rpc::spawn_stdio()?
            }
        };
        attempted = true;
        let result = rpc.call(
            "thread/queue/delete",
            json!({"threadId": peer.session_id, "queuedSubmissionId": queue_id}),
        )?;
        let object = result.as_object().ok_or(Failure::Class("AttributeError"))?;
        let Some(&Value::Bool(deleted)) = object.get("deleted") else {
            return Err(Failure::coded("codex_queue_delete_receipt_invalid"));
        };
        drop(rpc);
        Ok((deleted, queue_id))
    })();
    match result {
        Ok((deleted, queue_id)) => Cleanup::new(if deleted {
            CleanupStatus::Removed
        } else {
            CleanupStatus::Absent
        })
        .queue_id(queue_id),
        Err(failure) => Cleanup::new(if attempted || escapes(&failure) {
            CleanupStatus::Unknown
        } else {
            CleanupStatus::Unavailable
        })
        .detail(failure.to_string()),
    }
}

pub fn probe(peer: &Peer) -> Result<Value, Failure> {
    match socket(peer) {
        None => saved_identity(peer),
        Some(path) => {
            let mut rpc = Rpc::connect_unix(path)?;
            check_live(&mut rpc, peer)
        }
    }
}

fn socket(peer: &Peer) -> Option<&Path> {
    peer.socket
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(Path::new)
}

fn notify_socket(
    peer: &Peer,
    path: &Path,
    message_id: &str,
    body: &str,
    skip: Skip<'_>,
) -> Outcome {
    let mut attempted = false;
    let result = (|| -> Result<Outcome, Failure> {
        let mut rpc = Rpc::connect_unix(path)?;
        check_live(&mut rpc, peer)?;
        if let Some(reason) = skip()? {
            return Ok(Outcome::not_submitted(reason.as_str()));
        }
        attempted = true;
        let receipt = rpc.call(
            "thread/queue/add",
            json!({"threadId": peer.session_id,
            "clientUserMessageId": message_id, "input": [{"type": "text", "text": body}]}),
        )?;
        let queued = index(&receipt, "queuedSubmission")?;
        if index(queued, "clientUserMessageId")?.as_str() != Some(message_id) {
            return Err(Failure::coded("codex_queue_receipt_mismatch"));
        }
        drop(rpc);
        let id = index(queued, "id")?
            .as_str()
            .ok_or(Failure::Class("TypeError"))?;
        Ok(Outcome::submitted(format!("codex_queued:{id}")))
    })();
    result.unwrap_or_else(|failure| {
        if attempted || escapes(&failure) {
            Outcome::unknown(failure.to_string())
        } else {
            Outcome::not_submitted(failure.to_string())
        }
    })
}

fn notify_cli(peer: &Peer, body: &str, skip: Skip<'_>) -> Outcome {
    match saved_identity(peer).and_then(|_| skip()) {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok(Some(reason)) => return Outcome::not_submitted(reason.as_str()),
        Ok(None) => (),
    }
    let output = match os::run_command(
        "codex",
        &["queue", "--thread", &peer.session_id, "--message", body],
        QUEUE_TIMEOUT,
    ) {
        Ok(output) => output,
        // Spawn failures precede submission; anything after the process starts is uncertain.
        Err(failure @ Failure::Class("FileNotFoundError" | "PermissionError")) => {
            return Outcome::not_submitted(failure.to_string());
        }
        Err(failure) => return Outcome::unknown(failure.to_string()),
    };
    let stdout = match text_output(&output) {
        Ok(stdout) => stdout,
        Err(failure) => return Outcome::unknown(failure.to_string()),
    };
    static RECEIPT: OnceLock<regex::Regex> = OnceLock::new();
    let receipt = RECEIPT.get_or_init(|| {
        regex::Regex::new(
            r"\AQueued message ([0-9a-f-]{36}) for thread ([0-9a-f-]{36})\.[\s\x1c-\x1f]*\z",
        )
        .expect("valid receipt pattern")
    });
    match receipt.captures(&stdout) {
        Some(found) if output.status.code() == Some(0) && found[2] == peer.session_id => {
            match python_uuid(&found[1]) {
                Some(queue_id) => Outcome::submitted(format!("codex_cli_queued:{queue_id}")),
                None => Outcome::unknown("codex_cli_invalid_queue_id"),
            }
        }
        // The process may have queued before failing or returning an unfamiliar receipt.
        _ => Outcome::unknown("codex_cli_unconfirmed_receipt"),
    }
}

/// The live app-server address must be the registered thread, workspace and a loaded status.
fn check_live(rpc: &mut Rpc, peer: &Peer) -> Result<Value, Failure> {
    let result = rpc.call(
        "thread/read",
        json!({"threadId": peer.session_id, "includeTurns": false}),
    )?;
    let thread = index(&result, "thread")?
        .as_object()
        .ok_or(Failure::Class("AttributeError"))?;
    if thread.get("id").and_then(Value::as_str) != Some(peer.session_id.as_str())
        || !same_workspace(thread.get("cwd"), &peer.workspace)
    {
        return Err(Failure::coded("recipient_identity_changed"));
    }
    let status = match thread.get("status") {
        None => None,
        Some(Value::Object(status)) => status.get("type").and_then(Value::as_str),
        Some(_) => return Err(Failure::Class("AttributeError")),
    };
    let status = status
        .filter(|s| matches!(*s, "idle" | "active"))
        .ok_or_else(|| Failure::coded("recipient_not_loaded"))?;
    Ok(
        json!({"harness": "codex", "session_id": thread.get("id"), "workspace": thread.get("cwd"), "status": status}),
    )
}

/// Read the installed CLI's saved address, without starting any client.
fn saved_identity(peer: &Peer) -> Result<Value, Failure> {
    let path = state::state_path()?;
    let (id, cwd, archived, source) = state::saved_thread_at(&path, &peer.session_id)?
        .ok_or_else(|| Failure::coded("recipient_not_in_codex_state"))?;
    if id != peer.session_id || !same_workspace(Some(&cwd), &peer.workspace) {
        return Err(Failure::coded("recipient_identity_changed"));
    }
    if archived != 0 || source.as_str() != Some("cli") {
        return Err(Failure::coded(
            "recipient_is_not_an_unarchived_codex_cli_session",
        ));
    }
    Ok(
        json!({"harness": "codex", "session_id": id, "workspace": cwd,
        "metadata_source": path.to_string_lossy(), "transport": "codex_cli_queue", "runtime_status": "unknown"}),
    )
}

// ----- Helpers shared with the Claude transport -----

/// Failures the Python adapters did not catch; the store then recorded them as uncertain.
pub(crate) fn escapes(failure: &Failure) -> bool {
    matches!(
        failure,
        Failure::Class("AttributeError" | "KeyboardInterrupt")
    )
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

/// Compare a native path with the canonical workspace saved by registration.
pub(crate) fn same_workspace(directory: Option<&Value>, workspace: &str) -> bool {
    match directory {
        Some(Value::String(directory)) if Path::new(directory).is_absolute() => {
            os::resolve(Path::new(directory)).as_os_str() == std::ffi::OsStr::new(workspace)
        }
        _ => false,
    }
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

/// The recipient socket must be a Unix socket owned by this account.
pub(crate) fn owned_socket(path: &Path) -> Result<PathBuf, Failure> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let path = PathBuf::from(python_path(&path.to_string_lossy()));
    let info = std::fs::metadata(&path).map_err(io_failure)?;
    if !info.file_type().is_socket() || info.uid() != unsafe { libc::getuid() } {
        return Err(Failure::coded("recipient_socket_unavailable"));
    }
    Ok(path)
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

/// Connect a Unix stream socket within `timeout`, like Python's `settimeout` then `connect`.
pub(crate) fn connect_unix(
    path: &Path,
    timeout: Duration,
) -> io::Result<std::os::unix::net::UnixStream> {
    use std::os::{
        fd::FromRawFd,
        unix::{ffi::OsStrExt, net::UnixStream},
    };
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: plain socket syscalls on a descriptor owned by the returned UnixStream.
    unsafe {
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = UnixStream::from_raw_fd(fd);
        let mut address: libc::sockaddr_un = std::mem::zeroed();
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        if bytes.len() >= address.sun_path.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AF_UNIX path too long",
            ));
        }
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        let length =
            (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
        if libc::connect(fd, (&raw const address).cast(), length) != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            let milliseconds = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
            match libc::poll(&mut poll, 1, milliseconds) {
                0 => return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT)),
                n if n < 0 => return Err(io::Error::last_os_error()),
                _ => (),
            }
            let mut status: libc::c_int = 0;
            let mut size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            if libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut status).cast(),
                &mut size,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
        }
        stream.set_nonblocking(false)?;
        Ok(stream)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_compatible_helpers() {
        assert_eq!(
            python_dumps(&json!({"a": [1, "\u{e9}\n\u{1F600}\u{7f}"], "b": null})),
            r#"{"a": [1, "\u00e9\n\ud83d\ude00\u007f"], "b": null}"#
        );
        assert_eq!(python_uuid("{URN:0123}"), None);
        assert_eq!(
            python_uuid("urn:uuid:0123456789ABCDEF0123456789abcdef").as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
        assert_eq!(
            python_uuid("0123456789abcdef0123456789abcdef----").as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
        assert_eq!(python_uuid("invalid"), None);
        assert_eq!(python_path("/tmp//a/./b/"), "/tmp/a/b");
        assert_eq!(python_path("//a"), "//a");
        assert_eq!(python_path("///a"), "/a");
        assert_eq!(python_path(""), ".");
        assert_eq!(
            io_failure(io::Error::from_raw_os_error(libc::EISDIR)),
            Failure::Class("IsADirectoryError")
        );
        assert_eq!(
            socket_failure(io::Error::from_raw_os_error(libc::EAGAIN)),
            Failure::Class("TimeoutError")
        );
        assert!(!same_workspace(Some(&json!("relative")), "relative"));
        assert!(!same_workspace(Some(&json!(1)), "/"));
        assert!(same_workspace(Some(&json!("/")), "/"));
    }
}
