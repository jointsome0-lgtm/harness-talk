use crate::{
    cli::Context,
    commands::{self, Mailbox, PeerCommand},
    error::Error,
    model::{NativeSession, Peer, SkipReason},
    os, validate,
};
use serde_json::{Value, json};
use std::{env, path::Path};

pub fn command(db: &Path, actor: Option<&str>, parts: &[&str]) -> String {
    let resolved = os::resolve(db).to_string_lossy().into_owned();
    let mut words = vec!["htalk", "--db", &resolved];
    if let Some(actor) = actor.filter(|a| !a.is_empty()) {
        words.extend(["--as", actor]);
    }
    words.extend_from_slice(parts);
    os::shell_join(&words)
}

/// What to do about a message whose notice or whose removal is not settled. Any other message
/// is answered without advice.
pub fn message_actions(db: &Path, actor: &str, message: &mut Value) {
    let cleanup = message["notification_cleanup"]["status"].as_str();
    let stuck = message["recipient"] == actor
        && matches!(cleanup, Some("pending" | "unknown" | "unavailable"));
    let detail = message["notification_detail"].as_str();
    let unconfirmed = match message["submission"].as_str() {
        Some("submission_unknown") => true,
        // A notice that failed, and not one that was never due.
        Some("not_submitted") => detail.is_some_and(|d| SkipReason::parse(d).is_none()),
        _ => false,
    };
    if !stuck && !unconfirmed {
        return;
    }
    let id = message["id"].as_str().unwrap_or("");
    let mut recovery = json!({"show":command(db, Some(actor), &["show", id])});
    let mut action;
    if message["sender"] == actor && message["in_reply_to"].is_null() {
        if !message["reply"].is_null() {
            let answer = &message["reply"];
            let answer_id = answer["id"].as_str().unwrap_or("");
            recovery["show_reply"] = command(db, Some(actor), &["show", answer_id]).into();
            action = "Read the saved answer with recovery.show_reply.".to_owned();
            if answer["ack_at"].is_null() {
                recovery["ack_after_reading"] =
                    command(db, Some(actor), &["ack", answer_id]).into();
                action.push_str(" After reading, use recovery.ack_after_reading.");
            }
        } else {
            recovery["wait"] = command(db, Some(actor), &["wait", id, "--seconds", "45"]).into();
            action = "Use recovery.wait to wait again on this saved request, or recovery.show to inspect it.".to_owned();
        }
    } else if message["recipient"] == actor {
        action = "Read the message with recovery.show.".to_owned();
        if message["ack_at"].is_null() {
            recovery["ack_after_reading"] = command(db, Some(actor), &["ack", id]).into();
            action.push_str(" After reading, use recovery.ack_after_reading.");
        }
        if message["in_reply_to"].is_null() && message["reply"].is_null() {
            action.push_str(" Acknowledging a question leaves it open until you reply.");
        }
    } else {
        action = "Inspect the saved answer with recovery.show; the recipient can retrieve it from their inbox.".to_owned();
    }
    if stuck {
        recovery["retry_notification_cleanup"] = command(db, Some(actor), &["ack", id]).into();
        action.push_str(" Acknowledgment is saved. After submission finishes, use recovery.retry_notification_cleanup to retry removal.");
        if cleanup == Some("pending") {
            action.push_str(" If the sending process stopped before saving its completion receipt, cleanup can remain pending indefinitely; htalk cannot reconstruct the missing receipt.");
        }
        if cleanup == Some("unavailable") {
            action.push_str(" Verify the registered Codex address and native access. If sandbox restrictions prevented cleanup, use your client's normal approval flow before retrying.");
        }
    }
    action.push_str(" Never repeat an uncertain notification.");
    message["recovery"] = recovery;
    message["next_action"] = action.into();
}

/// The command for the page after this one, when there is one.
pub fn next_page(db: &Path, actor: &str, result: &mut Value, sent: bool, limit: i64, bodies: bool) {
    if result["omitted"].as_i64().unwrap_or(0) == 0 {
        return;
    }
    let Some(last) = result["messages"]
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|r| r["seq"].as_i64())
    else {
        return;
    };
    let last = last.to_string();
    let limit = limit.to_string();
    let mut parts = if sent {
        vec!["sent", "--limit", &limit, "--before-seq", &last]
    } else {
        vec!["inbox", "--limit", &limit, "--after-seq", &last]
    };
    if sent && bodies {
        parts.push("--bodies");
    }
    result["next_page"] = command(db, Some(actor), &parts).into();
}

/// What a failed catalogue command answers, and its exit code.
#[cfg(catalog)]
pub(crate) fn catalog_failure(error: &Error) -> (Value, i32) {
    let (action, code) = match error {
        Error::Interrupted => ("The command was interrupted. Run it again.", 130),
        _ => (
            "Correct what the error code names and run the command again. htalk catalog --help lists the commands.",
            2,
        ),
    };
    (
        json!({"state":"error", "error":error.to_string(), "next_action":action}),
        code,
    )
}

/// What a whole command answers on a system it is not ported to.
#[cfg(all(feature = "catalog", not(catalog)))]
pub(crate) fn not_ported() -> Value {
    let error = Error::not_ported();
    json!({"state":"error", "error":error.fixed(), "next_action":hint(error.fixed())})
}

/// The hint for an error code, where the code alone decides it. A code that is neither here
/// nor named in `explain` gets one of the two default hints at its end.
fn hint(code: &str) -> Option<&'static str> {
    Some(match code {
        "database_not_found" => {
            "No database file exists at resolved_path. Check --db and HTALK_DB; every participant must use the same file. Only peer add creates a database: see htalk peer add --help."
        }
        "database_backup_failed" => {
            "The backup could not be saved and verified; the mailbox was not migrated. Check free space, write access to backup_directory and SQLite integrity before retrying the same command. Existing backups are retained."
        }
        "database_foreign_key_violation" => {
            "Migration rolled back because the database contains broken references. Inspect PRAGMA foreign_key_check on a copy and repair the source or restore a valid backup before retrying. Do not change user_version manually."
        }
        "database_integrity_check_failed" => {
            "The mailbox was not migrated because SQLite's integrity check failed. Preserve the mailbox and any existing backups and inspect them before retrying. Do not change user_version manually."
        }
        "unsupported_unversioned_database" => {
            "No supported htalk schema version was found. The database was not changed. Verify that this is the intended mailbox and recover it with a matching client or a valid backup; do not assign a schema version manually."
        }
        // Any other failure of `migrate`.
        "migrate" => {
            "Migration did not report success. Inspect the error, schema version and database integrity on a copy before retrying. Do not change user_version manually."
        }
        "unsupported_harness" => {
            "Native notification adapters are codex, claude and opencode. For another harness, register with --delivery pull and poll inbox."
        }
        "unsupported_on_this_platform" => {
            "htalk does not do this on this operating system yet. A pull peer works on every system: register with --delivery pull and poll inbox."
        }
        "peer_retired" => {
            "Nothing was saved: new requests to or from a retired peer are refused. Choose an active peer with recovery.peers. Replies to saved requests still work. Restoring a peer is a registry decision, not a way to deliver this message."
        }
        _ => return None,
    })
}

pub(crate) fn interrupted(command: &Mailbox, context: &Context) -> Value {
    let mut recovery = json!({"peers":context.recovery(&["peer", "list"],false)});
    if context.own.is_some() {
        recovery["sent"] = context.recovery(&["sent"], true).into();
        recovery["inbox"] = context.recovery(&["inbox"], true).into();
    }
    let id = context
        .saved_id
        .as_deref()
        .or(command.message_id())
        .and_then(|s| validate::uuid(s).ok());
    let mut action = "Use the listed recovery commands to inspect the known ID or find saved messages. Do not resend.".to_owned();
    if let Some(id) = id.as_deref()
        && context.own.is_some()
    {
        recovery["show"] = context.recovery(&["show", id], true).into();
        if matches!(command, Mailbox::Ack { .. }) {
            recovery["retry_notification_cleanup"] = context.recovery(&["ack", id], true).into();
            action.push_str(" Use recovery.retry_notification_cleanup to finish acknowledgment and queue cleanup.");
        }
    }
    let mut value = json!({"state":"interrupted", "message_id":id, "recovery":recovery,
        "persistence":if context.saved_id.is_some() {"saved"} else {"unknown"}, "next_action":action});
    if let Some(source) = context.actor_source {
        value["actor_source"] = source.into();
    }
    value
}

fn retired_registration(context: &Context, value: &mut Value, peer: &Peer) {
    if let Some(retired) = peer.retired_at {
        value["retired_at"] = json!(retired);
        value["recovery"]["peers"] = context.recovery(&["peer", "list", "--all"], false).into();
        let action = value["next_action"].as_str().unwrap_or("");
        value["next_action"] = format!(
            "{action} Peer {} is retired, so recovery.peers includes retired peers.",
            peer.name
        )
        .into();
    }
}

/// What a failed command answers, and its exit code.
pub(crate) fn failure(command: &Mailbox, context: &Context, error: &Error) -> (Value, i32) {
    if matches!(error, Error::Interrupted) {
        return (interrupted(command, context), 130);
    }
    (explain(command, context, &error.text()), 2)
}

fn explain(command: &Mailbox, context: &Context, error: &str) -> Value {
    // A failure of the mailbox file says where the file is, and where its backups are.
    let backup = matches!(
        error,
        "database_backup_failed"
            | "database_foreign_key_violation"
            | "database_integrity_check_failed"
    );
    let file = backup || error == "database_not_found";
    if file || matches!(command, Mailbox::Migrate) {
        let path = os::resolve(&context.db);
        let mut value = json!({"state":"error", "error":error, "resolved_path":path});
        if backup {
            value["backup_directory"] = json!(crate::schema::backup_directory(&path));
        }
        let known = file || error == "unsupported_unversioned_database";
        value["next_action"] = hint(if known { error } else { "migrate" }).into();
        return value;
    }
    let mut value = json!({"state":"error", "error":error, "recovery":{"peers":context.recovery(&["peer","list"],false)}});
    let actor = context.actor.as_deref().unwrap_or("");
    match error {
        "unsupported_harness" | "unsupported_on_this_platform" => {
            value["next_action"] = hint(error).into()
        }
        "actor_conflicts_with_CODEX_THREAD_ID" | "actor_conflicts_with_CLAUDE_CODE_SESSION_ID" => {
            if let Some(own) = &context.own {
                let codex = error == "actor_conflicts_with_CODEX_THREAD_ID";
                let current = if codex {
                    env::var("CODEX_THREAD_ID").unwrap_or_default()
                } else if let Some(NativeSession::Recognized { session_id, .. }) = &context.native {
                    session_id.clone()
                } else {
                    String::new()
                };
                value["registered_peer"] = actor.into();
                value["registered_session_id"] = own.session_id.clone().into();
                value["current_session_id"] = current.clone().into();
                value["recovery"]["inbox_in_registered_session"] =
                    context.recovery(&["inbox"], true).into();
                value["next_action"] = if codex {
                    format!("Peer {actor} belongs to Codex session {}, not current session {current}. Use recovery.peers to find the peer registered to this session, or return to the registered session before running recovery.inbox_in_registered_session. The binding cannot be reassigned.",own.session_id.as_deref().unwrap_or(""))
                } else {
                    format!("Peer {actor} belongs to Claude session {}, not current session {current}. Check --as and HTALK_PEER. Use recovery.peers to find the peer registered to this session, which is selected when both are omitted, or return to the registered session before running recovery.inbox_in_registered_session. The binding cannot be reassigned.",own.session_id.as_deref().unwrap_or(""))
                }.into();
            }
        }
        "peer_required_use_as_or_HTALK_PEER" => {
            value["native_session"] = context
                .native
                .as_ref()
                .map(NativeSession::to_json)
                .unwrap_or(Value::Null);
            value["next_action"] = if matches!(context.native, Some(NativeSession::Recognized { .. })) {
                "This Claude Code session has no peer in this database. Register it with htalk peer add NAME --harness claude, using native_session.session_id and native_session.workspace, or use --as NAME. Inspect registered addresses with recovery.peers."
            } else {
                "Use --as NAME or set HTALK_PEER to your registered peer name. Only a recognized Claude Code session can omit both; native_session.reason names the check that failed. Inspect registered addresses with recovery.peers."
            }.into();
        }
        "peer_retired" => {
            let recipient = match command {
                Mailbox::Send { recipient, .. } => Some(recipient.as_str()),
                _ => None,
            };
            let names = [Some(actor), recipient]
                .into_iter()
                .flatten()
                .filter(|name| {
                    context
                        .store
                        .as_ref()
                        .and_then(|s| s.peer(name).ok())
                        .is_some_and(|p| p.retired_at.is_some())
                })
                .collect::<Vec<_>>();
            value["retired_peers"] = json!(names);
            value["next_action"] = hint(error).into();
        }
        "peer_already_has_a_different_address" | "session_already_has_a_peer_name" => {
            let peer = context.store.as_ref().and_then(|store| {
                let Mailbox::Peer(PeerCommand::Add(add)) = command else {
                    return None;
                };
                if error == "peer_already_has_a_different_address" {
                    store.peer(&add.name).ok()
                } else {
                    // The address as registration checked it a moment ago.
                    let wanted = commands::peer::registration(add).ok()?;
                    store
                        .session_peer(&wanted.harness, wanted.session_id.as_deref()?)
                        .ok()
                        .flatten()
                }
            });
            if let Some(peer) = peer {
                value["registered_peer"] = peer.name.clone().into();
                value["registered_session_id"] = peer.session_id.clone().into();
                value["next_action"] = if error == "peer_already_has_a_different_address" {
                    format!("Peer {} is already bound to {} session {}. Inspect recovery.peers. Keep that address for the existing session; a separate session needs a different peer name.",peer.name,peer.harness,peer.session_id.as_deref().unwrap_or("(pull)"))
                } else {
                    format!("Session {} already uses peer {}. Use that name from its registered session. Inspect recovery.peers for the existing immutable addresses.",peer.session_id.as_deref().unwrap_or("(pull)"),peer.name)
                }.into();
                retired_registration(context, &mut value, &peer);
                if error == "session_already_has_a_peer_name" && peer.retired_at.is_some() {
                    value["next_action"] = format!(
                        "{} To give this session new requests again, run htalk peer restore {}.",
                        value["next_action"].as_str().unwrap_or(""),
                        peer.name
                    )
                    .into();
                }
            }
        }
        "message_id_conflict" | "reply_conflict_existing_answer_preserved" => {
            let id = command.message_id();
            let id = if matches!(command, Mailbox::Send { .. }) {
                id.and_then(|id| validate::uuid(id).ok())
                    .unwrap_or_default()
            } else {
                id.unwrap_or("").to_owned()
            };
            value["message_id"] = id.clone().into();
            value["recovery"]["sent"] = context.recovery(&["sent"], true).into();
            if context
                .store
                .as_ref()
                .is_some_and(|store| store.get(&id, Some(actor)).is_ok())
            {
                value["recovery"]["show"] = context.recovery(&["show", &id], true).into();
                value["next_action"] = "The existing message was preserved. Inspect recovery.show or recover outgoing IDs with recovery.sent. Do not resend.".into();
            } else {
                value["next_action"] = "This ID is already in use and is not readable by this peer. Recover your own outgoing IDs with recovery.sent. Do not resend.".into();
            }
        }
        _ => {
            if context.own.is_some() {
                value["recovery"]["sent"] = context.recovery(&["sent"], true).into();
                value["recovery"]["inbox"] = context.recovery(&["inbox"], true).into();
                value["next_action"] = "Inspect saved messages with recovery.sent or recovery.inbox. Check registered addresses with recovery.peers. Never repeat an uncertain notification.".into();
            } else {
                value["next_action"] =
                    "Check the error and inspect registered addresses with recovery.peers.".into();
            }
        }
    }
    if let Some(source) = context.actor_source {
        value["actor_source"] = source.into();
    }
    // A failed `peer check` still says whether the peer it read is retired.
    if let Mailbox::Peer(PeerCommand::Check { name }) = command
        && let Some(peer) = context.store.as_ref().and_then(|s| s.peer(name).ok())
    {
        value["retired_at"] = json!(peer.retired_at);
    }
    value
}
