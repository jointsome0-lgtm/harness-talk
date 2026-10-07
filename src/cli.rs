use crate::{
    commands::{self, Cli, Command, Mailbox, PeerCommand, mail::To},
    error::Error,
    guidance, identity,
    mcp::Plain,
    model::{Delivery, NativeSession, Page, Peer},
    os,
    store::Store,
    validate,
};
use clap::{Parser, error::ErrorKind};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// A command's JSON answer and its exit code.
pub(crate) type Answer = Result<(Value, i32), Error>;

/// What every mailbox command works with. An error answer is built from what the command left here.
pub(crate) struct Context {
    pub db: PathBuf,
    pub actor: Option<String>,
    pub actor_source: Option<&'static str>,
    pub store: Option<Store>,
    pub own: Option<Peer>,
    pub native: Option<NativeSession>,
    pub saved_id: Option<String>,
}

/// An open mailbox and the registered peer a command acts as.
pub(crate) struct Session<'a> {
    pub db: &'a Path,
    pub store: &'a Store,
    pub own: &'a Peer,
    pub actor: &'a str,
    pub saved_id: &'a mut Option<String>,
    actor_source: Option<&'static str>,
}

impl Context {
    pub(crate) fn new(db: PathBuf, actor: Option<String>) -> Self {
        Self {
            db,
            actor,
            actor_source: None,
            store: None,
            own: None,
            native: None,
            saved_id: None,
        }
    }

    pub(crate) fn recovery(&self, parts: &[&str], actor: bool) -> String {
        guidance::command(
            &self.db,
            if actor { self.actor.as_deref() } else { None },
            parts,
        )
    }

    /// Opens the mailbox. Only `peer add` creates a missing one.
    pub(crate) fn open(&mut self, create: bool) -> Result<&Store, Error> {
        Ok(self.store.insert(Store::open(&self.db, create)?))
    }

    /// Opens the mailbox and settles who is calling: `--as`, then HTALK_PEER, then the
    /// peer registered for the Claude Code session that runs the command.
    pub(crate) fn session(&mut self) -> Result<Session<'_>, Error> {
        let store = &*self.store.insert(Store::open(&self.db, false)?);
        if self.actor.as_deref().is_some_and(|a| !a.is_empty()) {
            self.actor_source = Some("option");
        } else if let Some(actor) = env::var("HTALK_PEER").ok().filter(|a| !a.is_empty()) {
            self.actor = Some(actor);
            self.actor_source = Some("HTALK_PEER");
        } else {
            let native = native_session();
            if let NativeSession::Recognized { session_id, .. } = &native {
                self.own = store.session_peer("claude", session_id)?;
            }
            self.native = Some(native);
            let own = self
                .own
                .as_ref()
                .ok_or_else(|| Error::code("peer_required_use_as_or_HTALK_PEER"))?;
            self.actor = Some(own.name.clone());
            self.actor_source = Some("native_session");
        }
        let actor = self.actor.as_deref().unwrap_or_default();
        let own = match self.own.take() {
            Some(own) => own,
            None => store.peer(actor)?,
        };
        let own = &*self.own.insert(own);
        if let Some(id) = env::var("CODEX_THREAD_ID").ok().filter(|s| !s.is_empty())
            && own.delivery == Delivery::Native
            && own.harness == "codex"
            && own.session_id.as_deref() != Some(id.as_str())
        {
            return Err(Error::code("actor_conflicts_with_CODEX_THREAD_ID"));
        }
        if self.actor_source != Some("native_session")
            && own.delivery == Delivery::Native
            && own.harness == "claude"
        {
            let native = native_session();
            let conflict = matches!(&native, NativeSession::Recognized { session_id, .. } if Some(session_id.as_str()) != own.session_id.as_deref());
            self.native = Some(native);
            if conflict {
                return Err(Error::code("actor_conflicts_with_CLAUDE_CODE_SESSION_ID"));
            }
        }
        Ok(Session {
            db: &self.db,
            store,
            own,
            actor,
            saved_id: &mut self.saved_id,
            actor_source: self.actor_source,
        })
    }
}

impl Session<'_> {
    /// One message as an answer, with what its reader can do next.
    pub(crate) fn message(&self, mut value: Value) -> Value {
        guidance::message_actions(self.db, self.actor, &mut value);
        value["actor_source"] = json!(self.actor_source);
        value
    }

    /// A page of messages as an answer, each with what its reader can do next.
    pub(crate) fn page(
        &self,
        page: Page,
        sent: bool,
        limit: i64,
        bodies: bool,
    ) -> Result<Value, Error> {
        let mut value = serde_json::to_value(page)?;
        if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                guidance::message_actions(self.db, self.actor, message);
            }
        }
        guidance::page_actions(self.db, self.actor, &mut value, sent, limit, bodies);
        value["actor_source"] = json!(self.actor_source);
        Ok(value)
    }
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

fn execute(command: &Mailbox, context: &mut Context) -> Answer {
    match command {
        Mailbox::Peer(command) => commands::peer::run(command, context),
        Mailbox::Migrate => {
            Store::migrate(&context.db)?;
            Ok((
                json!({"state":"ready", "schema_version":crate::store::SCHEMA_VERSION,
                "resolved_path":os::resolve(&context.db)}),
                0,
            ))
        }
        Mailbox::Send {
            recipient,
            id,
            body,
            no_notify,
            wait,
        } => {
            let id = id.as_deref();
            commands::mail::write(context, To::Peer { recipient, id }, body, *no_notify, *wait)
        }
        Mailbox::Reply {
            message_id,
            body,
            no_notify,
        } => commands::mail::write(context, To::Request(message_id), body, *no_notify, 0.0),
        Mailbox::Wait {
            message_id,
            seconds,
        } => commands::mail::wait(context, message_id, *seconds),
        Mailbox::Show { message_id } => commands::read::show(context.session()?, message_id),
        Mailbox::Ack { message_id } => commands::read::ack(context.session()?, message_id),
        Mailbox::Inbox { limit, after_seq } => {
            commands::read::inbox(context.session()?, limit, after_seq.as_deref())
        }
        Mailbox::Watch => commands::read::watch(context.session()?),
        Mailbox::Sent {
            limit,
            before_seq,
            bodies,
        } => commands::read::sent(context.session()?, limit, before_seq.as_deref(), *bodies),
    }
}

fn interrupted(command: &Mailbox, context: &Context) -> Value {
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

fn failure(command: &Mailbox, context: &Context, error: &Error) -> Value {
    let error = error.to_string();
    if error == "database_not_found" {
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&context.db),
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
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&context.db),
            "backup_directory":crate::schema::backup_directory(&os::resolve(&context.db)), "next_action":action});
    }
    if matches!(command, Mailbox::Migrate) {
        let action = match error.as_str() {
            "unsupported_unversioned_database" => {
                "No supported htalk schema version was found. The database was not changed. Verify that this is the intended mailbox and recover it with a matching client or a valid backup; do not assign a schema version manually."
            }
            _ => {
                "Migration did not report success. Inspect the error, schema version and database integrity on a copy before retrying. Do not change user_version manually."
            }
        };
        return json!({"state":"error", "error":error, "resolved_path":os::resolve(&context.db), "next_action":action});
    }
    let mut value = json!({"state":"error", "error":error, "recovery":{"peers":context.recovery(&["peer","list"],false)}});
    let actor = context.actor.as_deref().unwrap_or("");
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
            value["next_action"] = "Nothing was saved: new requests to or from a retired peer are refused. Choose an active peer with recovery.peers. Replies to saved requests still work. Restoring a peer is a registry decision, not a way to deliver this message.".into();
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

pub(crate) fn output(value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let mut stdout = io::stdout().lock();
    stdout.write_all(&bytes)?;
    stdout.flush()
}

fn mcp(options: commands::Mcp, db: Option<String>, actor: Option<String>) -> i32 {
    let plain = if options.connect {
        if db.is_some() || actor.is_some() {
            eprintln!(
                "mcp --connect uses the remote endpoint's fixed database and peer; omit --db and --as"
            );
            return 2;
        }
        Plain::Connect(options.connector.clone())
    } else {
        let db = db.map(PathBuf::from).unwrap_or_else(default_db);
        let peer = actor
            .or_else(|| env::var("HTALK_PEER").ok())
            .unwrap_or_default();
        if let Err(error) = validate::peer_name(&peer) {
            eprintln!("htalk mcp requires --as NAME or HTALK_PEER: {error}");
            return 2;
        }
        Plain::Local { db, peer }
    };
    // The catalogue may bind the endpoint to what it published.
    #[cfg(feature = "catalog")]
    let result = crate::catalog::serve(plain, &options);
    #[cfg(not(feature = "catalog"))]
    let result = plain.serve();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("htalk mcp: {error}");
            2
        }
    }
}

pub fn main() -> i32 {
    let Cli {
        db, actor, command, ..
    } = match Cli::try_parse() {
        Ok(cli) => cli,
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
    let command = match command {
        #[cfg(feature = "catalog")]
        Command::Catalog(action) => {
            return crate::catalog::main(&action, db.as_deref(), actor.as_deref());
        }
        Command::Receive(receive) => {
            if db.is_some() || actor.is_some() {
                eprintln!("receive uses the remote watch binding; omit --db and --as");
                return 2;
            }
            return match crate::adapters::receive(receive) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("htalk receive: {error}");
                    2
                }
            };
        }
        Command::Mcp(options) => return mcp(options, db, actor),
        Command::Mailbox(command) => command,
    };
    unsafe {
        libc::umask(0o077);
    }
    let mut context = Context::new(db.map(PathBuf::from).unwrap_or_else(default_db), actor);
    let result = os::install_interrupt_handler()
        .map_err(Error::from)
        .and_then(|_| execute(&command, &mut context));
    let (value, code) = match result {
        // A migration that committed has a known result even if SIGINT arrived
        // immediately after commit. Failed/interrupted transactions still use 130.
        Ok(result) if matches!(command, Mailbox::Migrate) => result,
        _ if os::interrupted() => (interrupted(&command, &context), 130),
        Ok(result) => result,
        Err(Error::Interrupted) => (interrupted(&command, &context), 130),
        Err(error) => (failure(&command, &context, &error), 2),
    };
    if output(&value).is_err() {
        return 2;
    }
    code
}
