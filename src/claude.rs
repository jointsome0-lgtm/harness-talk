//! Claude Code: exact live identity from `claude agents --json` and one frame on its messaging socket.
use crate::codex::{
    connect_unix, escapes, index, io_failure, owned_socket, python_dumps, same_workspace,
    socket_failure, text_output,
};
use crate::{error::Failure, model::*, os};
use serde_json::{Map, Value, json};
use std::{io::Write, path::PathBuf, time::Duration};

const AGENTS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

/// The live session records. A non-list response is `invalid_claude_agents_response`.
pub fn agents() -> Result<Vec<Value>, Failure> {
    match agents_response()? {
        Value::Array(rows) => Ok(rows),
        _ => Err(Failure::coded("invalid_claude_agents_response")),
    }
}

/// `~/.claude/sessions/PID.json`, parsed but not yet verified.
pub fn session_metadata(pid: i64) -> Result<Value, Failure> {
    metadata_file(&pid.to_string())
}

/// The owned messaging socket of the one live session matching the peer's UUID and workspace.
pub fn live_socket(peer: &Peer) -> Result<PathBuf, Failure> {
    let listed = agents_response()?;
    let rows: Vec<&Map<String, Value>> = match &listed {
        Value::Array(rows) => rows
            .iter()
            .map(|row| row.as_object().ok_or(Failure::Class("AttributeError")))
            .collect::<Result<_, _>>()?,
        // Python iterated a mapping's keys or a string's characters, and each has no `.get`.
        Value::Object(map) if map.is_empty() => Vec::new(),
        Value::String(text) if text.is_empty() => Vec::new(),
        Value::Object(_) | Value::String(_) => return Err(Failure::Class("AttributeError")),
        _ => return Err(Failure::Class("TypeError")),
    };
    let matching: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            row.get("sessionId").and_then(Value::as_str) == Some(peer.session_id.as_str())
                && same_workspace(row.get("cwd"), &peer.workspace)
        })
        .collect();
    let pid = match matching.as_slice() {
        [row] => match row.get("pid") {
            Some(Value::Number(pid)) if pid.is_i64() || pid.is_u64() => pid.to_string(),
            _ => return Err(Failure::coded("recipient_unavailable")),
        },
        _ => return Err(Failure::coded("recipient_unavailable")),
    };
    let metadata = metadata_file(&pid)?;
    let fields = metadata
        .as_object()
        .ok_or(Failure::Class("AttributeError"))?;
    if fields.get("sessionId").and_then(Value::as_str) != Some(peer.session_id.as_str())
        || !same_workspace(fields.get("cwd"), &peer.workspace)
    {
        return Err(Failure::coded("recipient_identity_changed"));
    }
    let socket = index(&metadata, "messagingSocketPath")?
        .as_str()
        .ok_or(Failure::Class("TypeError"))?;
    owned_socket(std::path::Path::new(socket))
}

/// Write one frame after discovery and connection, unless the final state check says to skip.
/// A written frame proves only that the bytes reached the socket.
pub fn notify(peer: &Peer, message: &Message, body: &str, skip: Skip<'_>) -> Outcome {
    let path = match live_socket(peer) {
        Ok(path) => path,
        Err(failure) if escapes(&failure) => return Outcome::unknown(failure.to_string()),
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
    };
    let id = &message.row.id;
    let frame = json!({"type": "user", "session_id": peer.session_id, "uuid": id, "msg_id": id,
        "from": format!("htalk:{}", message.row.sender), "priority": "next",
        "message": {"role": "user", "content": body}});
    let mut connection = match connect_unix(&path, SOCKET_TIMEOUT) {
        Ok(connection) => connection,
        Err(error) => return Outcome::unknown(io_failure(error).to_string()),
    };
    if let Err(error) = connection.set_write_timeout(Some(SOCKET_TIMEOUT)) {
        return Outcome::unknown(io_failure(error).to_string());
    }
    match skip() {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok(Some(reason)) => return Outcome::not_submitted(reason.as_str()),
        Ok(None) => (),
    }
    match connection.write_all(format!("{}\n", python_dumps(&frame)).as_bytes()) {
        Ok(()) => Outcome::submitted("claude_socket_bytes_written"),
        Err(error) => Outcome::unknown(socket_failure(error).to_string()),
    }
}

pub fn probe(peer: &Peer) -> Result<Value, Failure> {
    let socket = live_socket(peer)?;
    Ok(
        json!({"harness": "claude", "session_id": peer.session_id, "workspace": peer.workspace,
        "socket": socket.to_string_lossy()}),
    )
}

fn agents_response() -> Result<Value, Failure> {
    let output = os::run_command("claude", &["agents", "--json"], AGENTS_TIMEOUT)?;
    let stdout = text_output(&output)?;
    if !output.status.success() {
        return Err(Failure::Class("CalledProcessError"));
    }
    serde_json::from_str(&stdout).map_err(|_| Failure::Class("JSONDecodeError"))
}

fn metadata_file(pid: &str) -> Result<Value, Failure> {
    let path = os::home()
        .join(".claude/sessions")
        .join(format!("{pid}.json"));
    let bytes = std::fs::read(path).map_err(io_failure)?;
    let text = String::from_utf8(bytes).map_err(|_| Failure::Class("UnicodeDecodeError"))?;
    serde_json::from_str(&text).map_err(|_| Failure::Class("JSONDecodeError"))
}
