//! Claude Code: exact live identity from `claude agents --json` and one frame on its messaging socket.
use super::{Adapter, Address, Query};
use crate::discovery::{address, source};
use crate::model::NativePeer as Peer;
use crate::os::{self, connect_unix, owned_socket, same_workspace};
use crate::{error::Error, model::*, validate};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

const AGENTS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct Claude;
impl Adapter for Claude {
    // The socket is never registered: each notification finds it from the live session.
    fn address(
        &self,
        session: &str,
        workspace: &str,
        socket: Option<&str>,
        url: Option<&str>,
    ) -> Result<Address, Error> {
        let session_id = validate::uuid(session)?;
        if url.is_some() {
            return Err(Error::code("url_is_only_for_opencode"));
        }
        let workspace = super::workspace(workspace)?;
        if socket.is_some() {
            return Err(Error::code(
                "claude_socket_is_discovered_from_live_identity",
            ));
        }
        Ok(Address {
            session_id,
            workspace,
            socket: None,
            url: None,
        })
    }
    fn notify(&self, peer: &Peer, message: &Message, body: &str, skip: Skip<'_>) -> Outcome {
        notify(peer, message, body, skip)
    }
    fn probe(&self, peer: &Peer) -> Result<Value, Error> {
        probe(peer)
    }
    fn discover(&self, _query: &Query<'_>) -> Found {
        sessions(agents(), &session_metadata)
    }
}

/// The live session records. A non-list response is `invalid_claude_agents_response`.
pub fn agents() -> Result<Vec<Value>, Error> {
    match agents_response()? {
        Value::Array(rows) => Ok(rows),
        _ => Err(invalid_agents()),
    }
}

fn invalid_agents() -> Error {
    Error::code("invalid_claude_agents_response")
}

/// `~/.claude/sessions/PID.json`, parsed but not yet verified.
pub fn session_metadata(pid: i64) -> Result<Value, Error> {
    metadata_file(&pid.to_string())
}

/// The owned messaging socket of the one live session matching the peer's UUID and workspace.
pub fn live_socket(peer: &Peer) -> Result<PathBuf, Error> {
    let listed = agents()?;
    let rows: Vec<&Map<String, Value>> = listed
        .iter()
        .map(|row| row.as_object().ok_or_else(invalid_agents))
        .collect::<Result<_, _>>()?;
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
            _ => return Err(Error::code("recipient_unavailable")),
        },
        _ => return Err(Error::code("recipient_unavailable")),
    };
    let metadata = metadata_file(&pid)?;
    let fields = metadata.as_object().ok_or(Error::invalid_data())?;
    if fields.get("sessionId").and_then(Value::as_str) != Some(peer.session_id.as_str())
        || !same_workspace(fields.get("cwd"), &peer.workspace)
    {
        return Err(Error::code("recipient_identity_changed"));
    }
    let socket = fields
        .get("messagingSocketPath")
        .and_then(Value::as_str)
        .ok_or(Error::invalid_data())?;
    owned_socket(Path::new(socket))
}

/// Write one frame after discovery and connection, unless the final state check says to skip.
/// A written frame proves only that the bytes reached the socket.
pub fn notify(peer: &Peer, message: &Message, body: &str, skip: Skip<'_>) -> Outcome {
    let path = match live_socket(peer) {
        Ok(path) => path,
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
    };
    let id = &message.row.id;
    let frame = json!({"type": "user", "session_id": peer.session_id, "uuid": id, "msg_id": id,
        "from": format!("htalk:{}", message.row.sender), "priority": "next",
        "message": {"role": "user", "content": body}});
    let mut connection = match connect_unix(&path, SOCKET_TIMEOUT) {
        Ok(connection) => connection,
        Err(error) => return Outcome::not_submitted(Error::from(error).to_string()),
    };
    if let Err(error) = connection.set_write_timeout(Some(SOCKET_TIMEOUT)) {
        return Outcome::not_submitted(Error::from(error).to_string());
    }
    match skip() {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok(Some(reason)) => return Outcome::not_submitted(reason.as_str()),
        Ok(None) => (),
    }
    match connection.write_all(format!("{frame}\n").as_bytes()) {
        Ok(()) => Outcome::submitted("claude_socket_bytes_written"),
        Err(error) => Outcome::unknown(Error::from(error).to_string()),
    }
}

pub fn probe(peer: &Peer) -> Result<Value, Error> {
    let socket = live_socket(peer)?;
    Ok(
        json!({"harness": "claude", "session_id": peer.session_id, "workspace": peer.workspace,
        "socket": socket.to_string_lossy()}),
    )
}

fn agents_response() -> Result<Value, Error> {
    let output = os::run_command("claude", &["agents", "--json"], AGENTS_TIMEOUT)?;
    let stdout = String::from_utf8(output.stdout).map_err(|_| Error::invalid_utf8())?;
    if !output.status.success() {
        return Err(Error::System("command_failed"));
    }
    Ok(serde_json::from_str(&stdout)?)
}

fn metadata_file(pid: &str) -> Result<Value, Error> {
    let path = os::home()
        .join(".claude/sessions")
        .join(format!("{pid}.json"));
    let text = String::from_utf8(std::fs::read(path)?).map_err(|_| Error::invalid_utf8())?;
    Ok(serde_json::from_str(&text)?)
}

/// Verify each `claude agents --json` row against its session metadata and live socket.
/// A malformed row is counted as rejected and never hides another verified row.
pub fn sessions(
    listed: Result<Vec<Value>, Error>,
    metadata: &dyn Fn(i64) -> Result<Value, Error>,
) -> Found {
    let mut src = source(json!({"harness": "claude", "source": "claude_agents", "status": "ok"}));
    let mut verified = Vec::new();
    match listed {
        Err(failure) => {
            src.insert("status".into(), json!("unavailable"));
            src.insert("detail".into(), json!(failure.fixed()));
        }
        Ok(rows) => {
            let mut identities: HashMap<String, usize> = HashMap::new();
            for row in &rows {
                if let Some(id) = row
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .and_then(|id| validate::uuid(id).ok())
                {
                    *identities.entry(id).or_default() += 1;
                }
            }
            let mut rejected = 0u64;
            for row in &rows {
                match verify_row(row, &identities, metadata) {
                    Some(session) => verified.push(session),
                    None => rejected += 1,
                }
            }
            if rejected > 0 {
                src.insert("status".into(), json!("partial"));
                src.insert("rejected".into(), json!(rejected));
                src.insert(
                    "detail".into(),
                    json!(
                        "Some live records could not be verified; run discovery again to refresh."
                    ),
                );
            }
        }
    }
    Found {
        sessions: verified,
        sources: vec![Value::Object(src)],
    }
}

fn verify_row(
    row: &Value,
    identities: &HashMap<String, usize>,
    metadata: &dyn Fn(i64) -> Result<Value, Error>,
) -> Option<Value> {
    let row = row.as_object()?;
    let (session_id, workspace) = address(Some(row.get("sessionId")?), Some(row.get("cwd")?))?;
    // Integers only: JSON floats and booleans are not PIDs.
    let pid = row.get("pid")?.as_i64().filter(|p| *p > 0)?;
    if identities.get(&session_id).copied() != Some(1) {
        return None;
    }
    let saved = metadata(pid).ok()?;
    let saved = saved.as_object()?;
    if saved.get("sessionId").and_then(Value::as_str) != Some(session_id.as_str())
        || saved.get("cwd").and_then(Value::as_str) != Some(workspace.as_str())
    {
        return None;
    }
    os::owned_socket(Path::new(saved.get("messagingSocketPath")?.as_str()?)).ok()?;
    Some(
        json!({"harness": "claude", "session_id": session_id, "workspace": workspace,
                "runtime_status": "running", "source": "claude_agents", "pid": pid}),
    )
}
