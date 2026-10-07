//! Codex notification, queue-receipt cleanup and probing.
//!
//! Only fixed htalk codes and exception-class names leave this module as details, so foreign
//! error text, paths and credentials never reach the shared database or command output.
pub mod discovery;
pub(crate) mod receive;
pub mod rpc;
pub mod state;

use super::{Adapter, Address, Query};
use crate::model::NativePeer as Peer;
use crate::os::{self, same_workspace};
use crate::{error::Error, model::*, validate};
use rpc::Rpc;
use serde_json::{Value, json};
use std::{path::Path, sync::OnceLock, time::Duration};

const QUEUE_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct Codex;
impl Adapter for Codex {
    // Without a socket the notification goes through the installed `codex` CLI.
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
        Ok(Address {
            session_id,
            workspace: super::workspace(workspace)?,
            socket: socket.map(|s| os::resolve(Path::new(s)).to_string_lossy().into_owned()),
            url: None,
        })
    }
    fn notify(&self, peer: &Peer, message: &Message, body: &str, skip: Skip<'_>) -> Outcome {
        notify(peer, &message.row.id, body, skip)
    }
    fn dismiss(&self, peer: &Peer, message: &Message) -> Cleanup {
        dismiss(peer, message)
    }
    fn probe(&self, peer: &Peer) -> Result<Value, Error> {
        probe(peer)
    }
    fn discover(&self, query: &Query<'_>) -> Found {
        discovery::discover(query.codex_sockets)
    }
}

pub fn notify(peer: &Peer, message_id: &str, body: &str, skip: Skip<'_>) -> Outcome {
    match socket(peer) {
        None => notify_cli(peer, body, skip, None),
        Some(path) => notify_socket(peer, path, message_id, body, skip, None),
    }
}

/// Keep receiver ownership in its captured store. CLI writes also pin that store;
/// an explicit server remains the delivery authority for socket writes.
pub(crate) fn notify_in_store(
    peer: &Peer,
    message_id: &str,
    body: &str,
    database: &Path,
    bound: Option<&rpc::BoundSocket>,
) -> Outcome {
    // sqlite_home selects state_5.sqlite; a differently named symlink target
    // cannot be pinned by passing its parent directory to the child CLI.
    if database.file_name() != Some(std::ffi::OsStr::new("state_5.sqlite")) {
        return Outcome::not_submitted("codex_store_name_unsupported");
    }
    let check = || {
        let current = std::fs::canonicalize(state::state_path()?)?;
        if current != database {
            return Err(Error::code("codex_store_changed"));
        }
        saved_identity_at(peer, database)?;
        Ok(None)
    };
    match bound {
        Some(bound) => notify_socket(peer, &bound.path, message_id, body, &check, Some(bound)),
        None => notify_cli(peer, body, &check, Some(database)),
    }
}

/// Delete only this message's saved Codex queue receipt. The caller has checked that its
/// recipient acknowledged it.
pub fn dismiss(peer: &Peer, message: &Message) -> Cleanup {
    let row = &message.row;
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
    let result = (|| -> Result<(bool, String), Error> {
        // Socket queue IDs are opaque protocol strings. CLI receipts have a UUID
        // contract; normalize only those, and round-trip socket receipts unchanged.
        let queue_id = if socket.is_some() {
            queued.to_owned()
        } else {
            validate::uuid(queued).map_err(|_| Error::code("codex_cli_invalid_queue_id"))?
        };
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
        let Some(&Value::Bool(deleted)) = result.get("deleted") else {
            return Err(Error::code("codex_queue_delete_receipt_invalid"));
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
        Err(failure) => Cleanup::new(if attempted {
            CleanupStatus::Unknown
        } else {
            CleanupStatus::Unavailable
        })
        .detail(failure.to_string()),
    }
}

pub fn probe(peer: &Peer) -> Result<Value, Error> {
    match socket(peer) {
        None => saved_identity(peer),
        Some(path) => {
            let mut rpc = Rpc::connect_unix(path)?;
            check_live(&mut rpc, peer)
        }
    }
}

/// Inspect only the selected listener and ensure it survived the identity read.
pub(crate) fn probe_bound(peer: &Peer, bound: &rpc::BoundSocket) -> Result<Value, Error> {
    let mut rpc = Rpc::connect_bound(bound)?;
    let value = check_live(&mut rpc, peer)?;
    bound.check()?;
    Ok(value)
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
    bound: Option<&rpc::BoundSocket>,
) -> Outcome {
    let mut attempted = false;
    let result = (|| -> Result<Outcome, Error> {
        let mut rpc = match bound {
            Some(bound) => Rpc::connect_bound(bound)?,
            None => Rpc::connect_unix(path)?,
        };
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
        let queued = receipt
            .get("queuedSubmission")
            .ok_or(Error::invalid_data())?;
        let client_id = queued
            .get("clientUserMessageId")
            .ok_or(Error::invalid_data())?;
        if client_id.as_str() != Some(message_id) {
            return Err(Error::code("codex_queue_receipt_mismatch"));
        }
        drop(rpc);
        let id = queued
            .get("id")
            .and_then(Value::as_str)
            .ok_or(Error::invalid_data())?;
        Ok(Outcome::submitted(format!("codex_queued:{id}")))
    })();
    result.unwrap_or_else(|failure| {
        if attempted {
            Outcome::unknown(failure.to_string())
        } else {
            Outcome::not_submitted(failure.to_string())
        }
    })
}

fn notify_cli(peer: &Peer, body: &str, skip: Skip<'_>, database: Option<&Path>) -> Outcome {
    let identity = match database {
        Some(path) => saved_identity_at(peer, path),
        None => saved_identity(peer),
    };
    match identity.and_then(|_| skip()) {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok(Some(reason)) => return Outcome::not_submitted(reason.as_str()),
        Ok(None) => (),
    }
    let pinned_store = match database {
        Some(path) => {
            let Some(parent) = path.parent().unwrap().to_str() else {
                return Outcome::not_submitted("codex_store_path_not_utf8");
            };
            Some(format!(
                "sqlite_home={}",
                toml::Value::String(parent.into())
            ))
        }
        None => None,
    };
    let mut args = vec!["queue"];
    if let Some(value) = &pinned_store {
        args.extend(["--config", value.as_str()]);
    }
    args.extend(["--thread", &peer.session_id, "--message", body]);
    let output = match os::run_command("codex", &args, QUEUE_TIMEOUT) {
        Ok(output) => output,
        // Spawn failures precede submission; anything after the process starts is uncertain.
        Err(failure) if matches!(failure.fixed(), "file_not_found" | "permission_denied") => {
            return Outcome::not_submitted(failure.to_string());
        }
        Err(failure) => return Outcome::unknown(failure.to_string()),
    };
    let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
        return Outcome::unknown(Error::invalid_utf8().to_string());
    };
    static RECEIPT: OnceLock<regex::Regex> = OnceLock::new();
    let receipt = RECEIPT.get_or_init(|| {
        regex::Regex::new(
            r"\AQueued message ([0-9a-f-]{36}) for thread ([0-9a-f-]{36})\.[\s\x1c-\x1f]*\z",
        )
        .expect("valid receipt pattern")
    });
    match receipt.captures(stdout) {
        Some(found) if output.status.code() == Some(0) && found[2] == peer.session_id => {
            match validate::uuid(&found[1]) {
                Ok(queue_id) => Outcome::submitted(format!("codex_cli_queued:{queue_id}")),
                Err(_) => Outcome::unknown("codex_cli_invalid_queue_id"),
            }
        }
        // The process may have queued before failing or returning an unfamiliar receipt.
        _ => Outcome::unknown("codex_cli_unconfirmed_receipt"),
    }
}

/// The live app-server address must be the registered thread, workspace and a loaded status.
fn check_live(rpc: &mut Rpc, peer: &Peer) -> Result<Value, Error> {
    let result = rpc.call(
        "thread/read",
        json!({"threadId": peer.session_id, "includeTurns": false}),
    )?;
    let thread = result
        .get("thread")
        .and_then(Value::as_object)
        .ok_or(Error::invalid_data())?;
    if thread.get("id").and_then(Value::as_str) != Some(peer.session_id.as_str())
        || !same_workspace(thread.get("cwd"), &peer.workspace)
    {
        return Err(Error::code("recipient_identity_changed"));
    }
    let status = match thread.get("status") {
        None => None,
        Some(Value::Object(status)) => status.get("type").and_then(Value::as_str),
        Some(_) => return Err(Error::invalid_data()),
    };
    let status = status
        .filter(|s| matches!(*s, "idle" | "active"))
        .ok_or_else(|| Error::code("recipient_not_loaded"))?;
    Ok(
        json!({"harness": "codex", "session_id": thread.get("id"), "workspace": thread.get("cwd"), "status": status}),
    )
}

/// Read the installed CLI's saved address, without starting any client.
fn saved_identity(peer: &Peer) -> Result<Value, Error> {
    let path = state::state_path()?;
    saved_identity_at(peer, &path)
}

fn saved_identity_at(peer: &Peer, path: &Path) -> Result<Value, Error> {
    let (id, cwd, archived, source) = state::saved_thread_at(path, &peer.session_id)?
        .ok_or_else(|| Error::code("recipient_not_in_codex_state"))?;
    if id != peer.session_id || !same_workspace(Some(&cwd), &peer.workspace) {
        return Err(Error::code("recipient_identity_changed"));
    }
    if archived != 0 || source.as_str() != Some("cli") {
        return Err(Error::code(
            "recipient_is_not_an_unarchived_codex_cli_session",
        ));
    }
    Ok(
        json!({"harness": "codex", "session_id": id, "workspace": cwd,
        "metadata_source": path.to_string_lossy(), "transport": "codex_cli_queue", "runtime_status": "unknown"}),
    )
}
