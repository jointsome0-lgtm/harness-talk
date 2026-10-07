use crate::{
    commands::{self, Cli, Command, Mailbox, mail::To},
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

fn native_session() -> NativeSession {
    identity::claude_session(&|key| env::var(key).ok(), Path::new(os::PROC), None, None)
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
        let db = db.map(PathBuf::from).unwrap_or_else(os::default_db);
        let peer = actor
            .or_else(|| env::var("HTALK_PEER").ok())
            .unwrap_or_default();
        if let Err(error) = validate::peer_name(&peer) {
            let error = if peer.is_empty() {
                Error::code("peer_required_use_as_or_HTALK_PEER")
            } else {
                error
            };
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
    os::private_umask();
    let mut context = Context::new(db.map(PathBuf::from).unwrap_or_else(os::default_db), actor);
    let result = os::install_interrupt_handler()
        .map_err(Error::from)
        .and_then(|_| execute(&command, &mut context));
    let (value, code) = match result {
        // A migration that committed has a known result even if an interrupt arrived
        // immediately after commit. Failed/interrupted transactions still use 130.
        Ok(result) if matches!(command, Mailbox::Migrate) => result,
        _ if os::interrupted() => (guidance::interrupted(&command, &context), 130),
        Ok(result) => result,
        Err(error) => guidance::failure(&command, &context, &error),
    };
    if output(&value).is_err() {
        return 2;
    }
    code
}
