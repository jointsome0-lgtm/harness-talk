use crate::{
    error::Error,
    guidance, identity,
    model::{Delivery, Harness, Message, NativeSession, Peer, SkipReason},
    notify, os,
    store::Store,
    validate,
};
use clap::{ArgMatches, error::ErrorKind};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::Duration,
};

struct Call {
    db: PathBuf,
    actor: Option<String>,
    command: String,
    peer_command: Option<String>,
    options: ArgMatches,
    message_body: Option<String>,
}
impl Call {
    fn new(mut matches: ArgMatches) -> Result<Self, Error> {
        let db = matches.remove_one::<String>("db").map(PathBuf::from);
        let db = db.unwrap_or_else(default_db);
        let actor = matches.remove_one::<String>("actor");
        let (command, mut options) = matches
            .remove_subcommand()
            .ok_or_else(|| Error::code("command_required"))?;
        let peer_command = if command == "peer" {
            let (name, sub) = options
                .remove_subcommand()
                .ok_or_else(|| Error::code("peer_command_required"))?;
            options = sub;
            Some(name)
        } else {
            None
        };
        Ok(Self {
            db,
            actor,
            command,
            peer_command,
            options,
            message_body: None,
        })
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.options
            .try_get_one::<String>(name)
            .ok()
            .flatten()
            .map(String::as_str)
    }
    fn required(&self, name: &str) -> Result<&str, Error> {
        self.value(name)
            .ok_or_else(|| Error::code("invalid_arguments"))
    }
    fn flag(&self, name: &str) -> bool {
        self.options
            .try_get_one::<bool>(name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
    }
    fn number(&self, name: &str) -> f64 {
        self.options
            .try_get_one::<f64>(name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(0.0)
    }
    fn repeated(&self, name: &str) -> Option<Vec<String>> {
        self.options
            .try_get_many::<String>(name)
            .ok()
            .flatten()
            .map(|v| v.cloned().collect())
    }
    fn recovery(&self, parts: &[&str], actor: bool) -> String {
        guidance::command(
            &self.db,
            if actor { self.actor.as_deref() } else { None },
            parts,
        )
    }
}

#[derive(Default)]
struct Context {
    turn: Option<std::fs::File>,
    store: Option<Store>,
    saved_id: Option<String>,
    own: Option<Peer>,
    native: Option<NativeSession>,
    actor_source: Option<&'static str>,
    checked: Option<Peer>,
}

fn default_db() -> PathBuf {
    if let Some(db) = env::var_os("HTALK_DB").filter(|v| !v.is_empty()) {
        return db.into();
    }
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| os::home().join(".local/share"))
        .join("harness-talk/mail.sqlite3")
}
fn native_session() -> NativeSession {
    identity::claude_session(&|key| env::var(key).ok(), Path::new("/proc"), None, None)
}
fn page_parameters(call: &Call, cursor: &str) -> Result<(i64, Option<i64>), Error> {
    let limit = call
        .value("limit")
        .unwrap_or("20")
        .parse::<i64>()
        .map_err(|_| Error::code("limit_must_be_between_1_and_500"))?;
    let cursor = call
        .value(cursor)
        .map(|v| {
            v.parse::<i64>()
                .map_err(|_| Error::code("seq_cursor_must_be_a_positive_integer"))
        })
        .transpose()?;
    validate::page(limit, cursor)?;
    Ok((limit, cursor))
}

fn execute(call: &mut Call, context: &mut Context) -> Result<(Value, i32), Error> {
    if !matches!(call.command.as_str(), "send" | "reply") {
        return execute_once(call, context);
    }
    let actor = call.actor.clone();
    let _probe = crate::write_turn::probe();
    let first = execute_once(call, context);
    let busy_before_write = matches!(&first, Err(Error::Db(rusqlite::Error::SqliteFailure(e, _)))
        if e.code == rusqlite::ErrorCode::DatabaseBusy)
        && crate::write_turn::fresh();
    if !busy_before_write || os::interrupted() {
        return first;
    }
    // The first attempt ended before any write transaction began. All its
    // read connections and snapshots are gone before waiting for the turn.
    *context = Context::default();
    call.actor = actor;
    context.turn = crate::write_turn::acquire(&call.db)?;
    crate::write_turn::queued(context.turn.is_some());
    if context.turn.is_some()
        && crate::write_turn::extra_successor_slot()
        && crate::store::await_writer(&call.db)
    {
        crate::write_turn::successor_grace();
    }
    let result = if os::interrupted() {
        Err(Error::Interrupted)
    } else {
        execute_once(call, context)
    };
    drop(context.turn.take());
    result
}

fn execute_once(call: &mut Call, context: &mut Context) -> Result<(Value, i32), Error> {
    if call.command == "peer" && call.peer_command.as_deref() == Some("discover") {
        let harness = call
            .value("harness")
            .map(str::parse::<Harness>)
            .transpose()?;
        let codex = call.repeated("codex_socket");
        let opencode = call.repeated("opencode_url");
        if codex.is_some() && !matches!(harness, None | Some(Harness::Codex)) {
            return Err(Error::code("codex_socket_requires_codex_discovery"));
        }
        if opencode.is_some() && !matches!(harness, None | Some(Harness::Opencode)) {
            return Err(Error::code("opencode_url_requires_opencode_discovery"));
        }
        let value = crate::discovery::discover(
            harness,
            call.value("workspace"),
            codex.as_deref(),
            opencode.as_deref(),
        );
        let failed = value["sources"]
            .as_array()
            .is_none_or(|sources| sources.iter().all(|s| s["status"] == "unavailable"));
        return Ok((value, if failed { 2 } else { 0 }));
    }
    if call.command == "send" {
        validate::wait(call.number("wait"))?;
    }
    if call.command == "wait" {
        validate::wait(call.number("seconds"))?;
    }
    if call.command == "migrate" {
        Store::migrate(&call.db)?;
        return Ok((
            json!({"state":"ready", "schema_version":crate::store::SCHEMA_VERSION,
            "resolved_path":os::resolve(&call.db)}),
            0,
        ));
    }
    let registration = if call.command == "peer" && call.peer_command.as_deref() == Some("add") {
        let name = call.required("name")?;
        let harness = call.required("harness")?;
        if call.value("delivery") == Some("pull") {
            if ["session", "workspace", "socket", "url"]
                .iter()
                .any(|key| call.value(key).is_some())
            {
                return Err(Error::code("pull_peer_has_no_native_address"));
            }
            let peer = Peer::pull(name, harness);
            validate::peer_name(name)?;
            validate::peer_name(harness).map_err(|_| Error::code("invalid_harness_id"))?;
            Some(peer)
        } else {
            let harness = harness.parse()?;
            Some(notify::native_peer(
                name,
                harness,
                call.required("session")?,
                call.required("workspace")?,
                call.value("socket"),
                call.value("url"),
            )?)
        }
    } else {
        None
    };
    context.store = Some(Store::open(
        &call.db,
        call.command == "peer" && call.peer_command.as_deref() == Some("add"),
    )?);
    let store = context
        .store
        .as_ref()
        .ok_or_else(|| Error::code("database_not_found"))?;
    if call.command == "peer" {
        let value = match call.peer_command.as_deref() {
            Some("add") => {
                let name = call.required("name")?;
                let peer = store.register(
                    registration
                        .as_ref()
                        .ok_or_else(|| Error::code("invalid_arguments"))?,
                )?;
                let mut value = serde_json::to_value(&peer)?;
                if peer.retired_at.is_some() {
                    value["recovery"] =
                        json!({"restore":call.recovery(&["peer", "restore", name], false)});
                    value["next_action"] = format!("Peer {name} is retired, and peer add does not change that. If this session should get new requests again, use recovery.restore.").into();
                }
                value
            }
            Some("check") => {
                let peer = store.peer(call.required("name")?)?;
                context.checked = Some(peer.clone());
                let mut value = notify::probe(&peer)?;
                value["retired_at"] = json!(peer.retired_at);
                value
            }
            Some("retire") => serde_json::to_value(store.retire(call.required("name")?)?)?,
            Some("restore") => serde_json::to_value(store.restore(call.required("name")?)?)?,
            Some("list") => {
                let peers = store.peers()?;
                let total = peers.len();
                let visible: Vec<_> = peers
                    .into_iter()
                    .filter(|p| call.flag("all") || p.retired_at.is_none())
                    .collect();
                let hidden = total - visible.len();
                let mut value = json!({"peers": visible});
                if !call.flag("all") {
                    value["retired_hidden"] = hidden.into();
                }
                value
            }
            _ => return Err(Error::code("invalid_arguments")),
        };
        return Ok((value, 0));
    }
    if call.actor.as_deref().is_some_and(|a| !a.is_empty()) {
        context.actor_source = Some("option");
    } else if let Some(actor) = env::var("HTALK_PEER").ok().filter(|a| !a.is_empty()) {
        call.actor = Some(actor);
        context.actor_source = Some("HTALK_PEER");
    } else {
        let native = native_session();
        if let NativeSession::Recognized { session_id, .. } = &native {
            context.own = store.session_peer("claude", session_id)?;
        }
        context.native = Some(native);
        let own = context
            .own
            .as_ref()
            .ok_or_else(|| Error::code("peer_required_use_as_or_HTALK_PEER"))?;
        call.actor = Some(own.name.clone());
        context.actor_source = Some("native_session");
    }
    let actor = call
        .actor
        .as_deref()
        .ok_or_else(|| Error::code("peer_required_use_as_or_HTALK_PEER"))?;
    if context.own.is_none() {
        context.own = Some(store.peer(actor)?);
    }
    let own = context
        .own
        .as_ref()
        .ok_or_else(|| Error::code("unknown_peer"))?;
    if let Some(id) = env::var("CODEX_THREAD_ID").ok().filter(|s| !s.is_empty())
        && own.delivery == Delivery::Native
        && own.harness == "codex"
        && own.session_id.as_deref() != Some(id.as_str())
    {
        return Err(Error::code("actor_conflicts_with_CODEX_THREAD_ID"));
    }
    if context.actor_source != Some("native_session")
        && own.delivery == Delivery::Native
        && own.harness == "claude"
    {
        let native = native_session();
        let conflict = matches!(&native, NativeSession::Recognized { session_id, .. } if Some(session_id.as_str()) != own.session_id.as_deref());
        context.native = Some(native);
        if conflict {
            return Err(Error::code("actor_conflicts_with_CLAUDE_CODE_SESSION_ID"));
        }
    }
    let mut attempted_notification = false;
    let mut value = match call.command.as_str() {
        "watch" => return watch(store, own, &call.db),
        "send" | "reply" => {
            let body = if let Some(body) = &call.message_body {
                body.clone()
            } else {
                let body = if let Some(path) = call.value("message_file") {
                    std::fs::read_to_string(path)?
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                } else {
                    call.required("message")?.to_owned()
                };
                call.message_body = Some(body.clone());
                body
            };
            let (recipient, reply_to, id) = if call.command == "reply" {
                let request_id = call.required("message_id")?;
                let request = store.get(request_id, Some(actor))?;
                (request.row.sender, Some(request_id), None)
            } else {
                (
                    call.required("recipient")?.to_owned(),
                    None,
                    call.value("id"),
                )
            };
            let (mut message, created) = store.save(actor, &recipient, &body, id, reply_to)?;
            crate::write_turn::started_write();
            drop(context.turn.take());
            context.saved_id = Some(message.row.id.clone());
            if os::interrupted() {
                return Err(Error::Interrupted);
            }
            if created && !call.flag("no_notify") {
                attempted_notification = true;
                let database = store.path();
                message = store.notify_once(
                    &message.row.id,
                    &|p, m| notify::notify(p, m, database),
                    &notify::dismiss,
                )?;
            }
            if os::interrupted() {
                return Err(Error::Interrupted);
            }
            if call.command == "send" && call.number("wait") != 0.0 {
                message = store.wait(&message.row.id, actor, call.number("wait"))?;
            }
            let mut value = serde_json::to_value(message)?;
            value["created"] = created.into();
            value
        }
        "wait" => serde_json::to_value(store.wait(
            call.required("message_id")?,
            actor,
            call.number("seconds"),
        )?)?,
        "show" => serde_json::to_value(store.get(call.required("message_id")?, Some(actor))?)?,
        "ack" => serde_json::to_value(store.ack(
            call.required("message_id")?,
            actor,
            &notify::dismiss,
        )?)?,
        "inbox" => {
            let (limit, cursor) = page_parameters(call, "after_seq")?;
            serde_json::to_value(store.inbox(actor, limit, cursor)?)?
        }
        "sent" => {
            let (limit, cursor) = page_parameters(call, "before_seq")?;
            serde_json::to_value(store.sent(actor, limit, cursor, call.flag("bodies"))?)?
        }
        _ => return Err(Error::code("invalid_arguments")),
    };
    if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            guidance::message_actions(&call.db, actor, message);
        }
        let (limit, _) = page_parameters(
            call,
            if call.command == "sent" {
                "before_seq"
            } else {
                "after_seq"
            },
        )?;
        guidance::page_actions(
            &call.db,
            actor,
            &mut value,
            call.command == "sent",
            limit,
            call.flag("bodies"),
        );
    } else {
        guidance::message_actions(&call.db, actor, &mut value);
    }
    value["actor_source"] = json!(context.actor_source);
    let unconfirmed = attempted_notification
        && value["reply"].is_null()
        && value["submission"] != "submitted"
        && value["ack_at"].is_null()
        && value["notification_detail"]
            .as_str()
            .and_then(SkipReason::parse)
            .is_none();
    Ok((value, if unconfirmed { 2 } else { 0 }))
}

fn interrupted(call: &Call, context: &Context) -> Value {
    let mut recovery = json!({"peers":call.recovery(&["peer", "list"],false)});
    if context.own.is_some() {
        recovery["sent"] = call.recovery(&["sent"], true).into();
        recovery["inbox"] = call.recovery(&["inbox"], true).into();
    }
    let id = context
        .saved_id
        .as_deref()
        .or(call.value("message_id"))
        .or(call.value("id"))
        .and_then(|s| validate::uuid(s).ok());
    let mut action = "Use the listed recovery commands to inspect the known ID or find saved messages. Do not resend.".to_owned();
    if let Some(id) = id.as_deref()
        && context.own.is_some()
    {
        recovery["show"] = call.recovery(&["show", id], true).into();
        if call.command == "ack" {
            recovery["retry_notification_cleanup"] = call.recovery(&["ack", id], true).into();
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

fn retired_registration(call: &Call, value: &mut Value, peer: &Peer) {
    if let Some(retired) = peer.retired_at {
        value["retired_at"] = json!(retired);
        value["recovery"]["peers"] = call.recovery(&["peer", "list", "--all"], false).into();
        let action = value["next_action"].as_str().unwrap_or("");
        value["next_action"] = format!(
            "{action} Peer {} is retired, so recovery.peers includes retired peers.",
            peer.name
        )
        .into();
    }
}

fn failure(call: &Call, context: &Context, error: &Error) -> Value {
    let error = error.to_string();
    if error == "database_not_found" {
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&call.db),
            "next_action":"No database file exists at resolved_path. Check --db and HTALK_DB; every participant must use the same file. Only peer add creates a database: see htalk peer add --help."});
    }
    if matches!(
        error.as_str(),
        "database_backup_failed"
            | "database_foreign_key_violation"
            | "database_integrity_check_failed"
    ) {
        let action = match error.as_str() {
            "database_backup_failed" => {
                "The backup could not be saved and verified; the mailbox was not migrated. Check free space, write access to backup_directory and SQLite integrity before retrying the same command. Existing backups are retained."
            }
            "database_foreign_key_violation" => {
                "Migration rolled back because the database contains broken references. Inspect PRAGMA foreign_key_check on a copy and repair the source or restore a valid backup before retrying. Do not change user_version manually."
            }
            _ => {
                "The mailbox was not migrated because SQLite's integrity check failed. Preserve the mailbox and any existing backups and inspect them before retrying. Do not change user_version manually."
            }
        };
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&call.db),
            "backup_directory":crate::schema::backup_directory(&os::resolve(&call.db)), "next_action":action});
    }
    if call.command == "migrate" {
        let action = match error.as_str() {
            "unsupported_unversioned_database" => {
                "No supported htalk schema version was found. The database was not changed. Verify that this is the intended mailbox and recover it with a matching client or a valid backup; do not assign a schema version manually."
            }
            _ => {
                "Migration did not report success. Inspect the error, schema version and database integrity on a copy before retrying. Do not change user_version manually."
            }
        };
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&call.db), "next_action":action});
    }
    let mut value = json!({"state":"error", "error":error, "recovery":{"peers":call.recovery(&["peer","list"],false)}});
    let actor = call.actor.as_deref().unwrap_or("");
    match error.as_str() {
        "unsupported_harness" => {
            value["next_action"] = "Native notification adapters are codex, claude and opencode. For another harness, register with --delivery pull and poll inbox.".into();
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
                    call.recovery(&["inbox"], true).into();
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
            let names = [Some(actor), call.value("recipient")]
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
            value["next_action"] = "Nothing was saved: new requests to or from a retired peer are refused. Choose an active peer with recovery.peers. Replies to saved requests still work. Restoring a peer is a registry decision, not a way to deliver this message.".into();
        }
        "peer_already_has_a_different_address" | "session_already_has_a_peer_name" => {
            let peer = context.store.as_ref().and_then(|store| {
                if error == "peer_already_has_a_different_address" {
                    store.peer(call.value("name")?).ok()
                } else {
                    let harness: Harness = call.value("harness")?.parse().ok()?;
                    let session = call.value("session")?;
                    let session = if harness == Harness::Opencode {
                        session.to_owned()
                    } else {
                        validate::uuid(session).ok()?
                    };
                    store
                        .session_peer(harness.as_str(), &session)
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
                retired_registration(call, &mut value, &peer);
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
            let id = if call.command == "send" {
                call.value("id")
                    .and_then(|id| validate::uuid(id).ok())
                    .unwrap_or_default()
            } else {
                call.value("message_id").unwrap_or("").to_owned()
            };
            value["message_id"] = id.clone().into();
            value["recovery"]["sent"] = call.recovery(&["sent"], true).into();
            if context
                .store
                .as_ref()
                .is_some_and(|store| store.get(&id, Some(actor)).is_ok())
            {
                value["recovery"]["show"] = call.recovery(&["show", &id], true).into();
                value["next_action"] = "The existing message was preserved. Inspect recovery.show or recover outgoing IDs with recovery.sent. Do not resend.".into();
            } else {
                value["next_action"] = "This ID is already in use and is not readable by this peer. Recover your own outgoing IDs with recovery.sent. Do not resend.".into();
            }
        }
        _ => {
            if context.own.is_some() {
                value["recovery"]["sent"] = call.recovery(&["sent"], true).into();
                value["recovery"]["inbox"] = call.recovery(&["inbox"], true).into();
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
    if let Some(checked) = &context.checked {
        value["retired_at"] = json!(checked.retired_at);
    }
    value
}

fn output(value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let mut stdout = io::stdout().lock();
    stdout.write_all(&bytes)?;
    stdout.flush()
}

fn watch(store: &Store, peer: &Peer, db: &Path) -> Result<(Value, i32), Error> {
    let mut after_seq = None;
    output(&json!({"event":"ready", "peer":peer.name}))?;
    loop {
        if os::interrupted() {
            return Err(Error::Interrupted);
        }
        // An unloaded/crashed extension closes its read end, even if no new
        // mail arrives. Do not leave an idle watcher behind in that case.
        let mut sink = libc::pollfd {
            fd: io::stdout().as_raw_fd(),
            events: 0,
            revents: 0,
        };
        if unsafe { libc::poll(&mut sink, 1, 0) } > 0
            && sink.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
        {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe).into());
        }
        let page = match store.inbox(&peer.name, 100, after_seq) {
            Ok(page) => page,
            Err(Error::Db(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Err(error) => return Err(error),
        };
        for value in page.messages {
            let message: Message = serde_json::from_value(value)?;
            output(&json!({"event":"message", "id":message.row.id,
                "seq":message.row.seq, "notification":notify::notification(peer, &message, db)}))?;
            after_seq = Some(message.row.seq);
        }
        if page.omitted == 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

pub fn main() -> i32 {
    let matches = match crate::parser::command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            if error.kind() == ErrorKind::DisplayVersion {
                return if writeln!(io::stdout().lock(), "{}", env!("CARGO_PKG_VERSION")).is_ok() {
                    0
                } else {
                    2
                };
            }
            let code = if error.kind() == ErrorKind::DisplayHelp {
                0
            } else {
                2
            };
            let _ = error.print();
            return code;
        }
    };
    if matches.subcommand_name() == Some("mcp") {
        let db = matches
            .get_one::<String>("db")
            .map(PathBuf::from)
            .unwrap_or_else(default_db);
        let peer = matches
            .get_one::<String>("actor")
            .cloned()
            .or_else(|| env::var("HTALK_PEER").ok())
            .unwrap_or_default();
        if let Err(error) = validate::peer_name(&peer) {
            eprintln!("htalk mcp requires --as NAME or HTALK_PEER: {error}");
            return 2;
        }
        return match crate::mcp::run(db, peer) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("htalk mcp: {error}");
                2
            }
        };
    }
    let mut call = match Call::new(matches) {
        Ok(call) => call,
        Err(error) => {
            let _ = output(&json!({"state":"error","error":error.to_string()}));
            return 2;
        }
    };
    unsafe {
        libc::umask(0o077);
    }
    let mut context = Context::default();
    let result = os::install_interrupt_handler()
        .map_err(Error::from)
        .and_then(|_| execute(&mut call, &mut context));
    let (value, code) = match result {
        // A migration that committed has a known result even if SIGINT arrived
        // immediately after commit. Failed/interrupted transactions still use 130.
        Ok(result) if call.command == "migrate" => result,
        _ if os::interrupted() => (interrupted(&call, &context), 130),
        Ok(result) => result,
        Err(Error::Interrupted) => (interrupted(&call, &context), 130),
        Err(error) => (failure(&call, &context, &error), 2),
    };
    if output(&value).is_err() {
        return 2;
    }
    code
}
