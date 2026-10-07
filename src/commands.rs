//! The command table. Every command is declared here once, with its arguments and help text;
//! the parser, the dispatch in `cli.rs` and the MCP tool read this declaration.
use clap::{ArgAction, Args, Parser, Subcommand};
use rmcp::schemars;

pub(crate) mod mail;
pub(crate) mod peer;
pub(crate) mod read;

// Keep integer overflow for semantic validation, which returns a JSON error.
fn integer_argument(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
        Ok(trimmed.to_owned())
    } else {
        Err(format!("invalid integer value: {value:?}"))
    }
}

// `arg_required_else_help = false`, here and on the two groups below, keeps a missing
// command the usage error it was; the derive would print the whole help instead.
#[derive(Parser)]
#[command(
    name = "htalk",
    version,
    disable_version_flag = true,
    disable_help_subcommand = true,
    arg_required_else_help = false,
    about = "htalk: durable local messages with optional client notifications.",
    after_help = "Setup: use one shared database and register both participants.\n  htalk peer discover\n  htalk peer add --help\n\nExchange, using each session's own registered name:\n  htalk --as alice send bob --message 'Please check this.' --wait 45\n  htalk --as bob inbox\n  htalk --as bob ack REQUEST_ID\n  htalk --as bob reply REQUEST_ID --message 'Checked.'\n  htalk --as alice wait REQUEST_ID\n  htalk --as alice ack REPLY_ID\nRead the body before ack. REQUEST_ID and REPLY_ID are message IDs from JSON,\nnot native session IDs. A reply has its own id and an in_reply_to request ID.\n\nPut --db and --as before the command, or set HTALK_DB and HTALK_PEER.\nA command run by a registered Claude Code session can omit --as.\nUse htalk COMMAND --help, or htalk peer COMMAND --help, for examples.\nResults are JSON; one that failed or is uncertain adds next_action and recovery.\nExit 0: completed, including send --wait that returned an answer. Exit 2:\ninvalid input, or an unconfirmed notification without an answer; the message\nmay be saved. Exit 130: interrupted; inspect recovery."
)]
pub(crate) struct Cli {
    #[arg(long, action = ArgAction::Version, help = "show program's version number and exit")]
    version: Option<bool>,
    #[arg(
        long,
        value_name = "PATH",
        help = crate::os::DB_HELP
    )]
    pub db: Option<String>,
    #[arg(
        long = "as",
        value_name = "NAME",
        help = "Your registered peer name; overrides HTALK_PEER. Without either, message commands use the peer registered for the Claude Code session running them, if recognized."
    )]
    pub actor: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    #[cfg(feature = "catalog")]
    #[command(
        subcommand,
        arg_required_else_help = false,
        about = "Publish profiles and find known devices through pinned SSH, LAN, active Bluetooth PAN or Tailscale.",
        long_about = "Only explicitly published profiles are exported. Device discovery is unauthenticated; profile reads and mailbox calls require pinned SSH identity and a fixed endpoint. Never launches sessions or retries messages."
    )]
    Catalog(crate::catalog::Action),
    #[command(
        about = "Expose this peer's mailbox tools over MCP stdio.",
        long_about = "Run a local MCP stdio server with a fixed database and peer, or keep a client connection alive while connecting to a remote mailbox separately for each tool call. Local mode requires --as or HTALK_PEER. Remote mode uses --connect -- COMMAND ARGS; that owner-configured command must reach a fixed htalk MCP endpoint. Calls are never automatically retried. This server does not wake an idle agent."
    )]
    Mcp(Mcp),
    #[command(
        subcommand_negates_reqs = true,
        args_conflicts_with_subcommands = true,
        about = "Forward a remote watch stream to an existing Codex CLI session.",
        long_about = "Keep one existing Codex CLI session informed by a fixed remote htalk watch command. The receiver reconnects read-only watches and durably records each native queue receipt. Uncertain submissions stop for inspection. It does not launch a worker, acknowledge mail, or prove task completion. Use that session's remote htalk MCP tool for mailbox access."
    )]
    Receive(Receive),
    #[command(flatten)]
    Mailbox(Mailbox),
}

#[derive(Args)]
pub(crate) struct Mcp {
    #[arg(
        long,
        requires = "connector",
        help = "Connect to a fixed remote htalk MCP endpoint per call. Put its command after --; do not pass local --db or --as."
    )]
    pub connect: bool,
    #[arg(
        last = true,
        num_args = 1..,
        requires = "connect",
        value_name = "connector",
        help = "Owner-configured executable and arguments; executed directly, without a shell."
    )]
    pub connector: Vec<String>,
    #[cfg(feature = "catalog")]
    #[arg(
        long,
        conflicts_with = "connect",
        value_name = "catalog",
        help = "Expose the fixed published catalogue binding in the MCP handshake; local mode only."
    )]
    pub catalog: Option<String>,
    #[cfg(feature = "catalog")]
    #[arg(
        long,
        requires = "connect",
        value_name = "expect_catalog",
        help = "Private expected catalogue binding; reject a changed endpoint before every tool call."
    )]
    pub expect_catalog: Option<String>,
}

#[derive(Args)]
pub(crate) struct Receive {
    #[command(subcommand)]
    pub action: Option<ReceiveAction>,
    #[command(flatten)]
    pub watch: Option<ReceiveWatch>,
}

#[derive(Subcommand)]
pub(crate) enum ReceiveAction {
    #[command(about = "Inspect saved receipts and the local Codex target without waking it.")]
    Status {
        #[arg(long, value_name = "state")]
        state: String,
    },
    #[command(
        about = "Bind stopped receiver state to a replacement socket hosting the same session.",
        long_about = "After explicitly resuming the same Codex session at the saved socket path, verify its UUID and workspace and accept the replacement listener. Preserves receipts; refuses active receivers and unresolved submissions. Does not resume, queue, or resend work."
    )]
    Rebind {
        #[arg(long, value_name = "state")]
        state: String,
    },
}

#[derive(Args)]
pub(crate) struct ReceiveWatch {
    #[arg(long, value_name = "peer")]
    pub peer: String,
    #[arg(long, value_name = "session")]
    pub session: String,
    #[arg(long, value_name = "workspace")]
    pub workspace: String,
    #[arg(
        long,
        value_name = "state",
        help = "Private receiver directory. Keep it across reconnects and restarts."
    )]
    pub state: String,
    #[arg(
        long,
        value_name = "PATH",
        help = "Absolute executable for the same fixed MCP mailbox. Recheck each message before waking; no arguments or shell. Keep this binding across restarts."
    )]
    pub mcp_command: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Explicit local Codex app-server Unix socket hosting this session. Use its existing native adapter instead of codex queue; never fall back to another server."
    )]
    pub codex_socket: Option<String>,
    #[arg(
        last = true,
        num_args = 1..,
        required = true,
        value_name = "connector",
        help = "Fixed watch executable and arguments, after --; no shell."
    )]
    pub connector: Vec<String>,
}

// The commands that work on one mailbox and answer in JSON.
#[derive(Subcommand)]
pub(crate) enum Mailbox {
    #[command(
        subcommand,
        arg_required_else_help = false,
        about = "Discover, register, check and retire session addresses.",
        long_about = "Discover sessions and manage registered addresses. These commands do not message peers.",
        after_help = "Start with htalk peer discover, then htalk peer add --help.\nAfter registering: htalk peer check NAME\nWhen a session is no longer used: htalk peer retire NAME\nUse the same --db PATH before peer for both sessions."
    )]
    Peer(PeerCommand),
    #[command(
        about = "Prepare this mailbox without another operation.",
        long_about = "Ordinary mailbox commands automatically back up and upgrade schema 1/2 on first use. This optional command runs the same preparation on the database selected by --db, HTALK_DB or the default location. Verified backups are kept in PATH.backups. Older binaries reject schema 3. A failed migration rolls back; no automatic downgrade or backup restoration is performed."
    )]
    Migrate,
    #[command(
        about = "Save a request and attempt one notification.",
        long_about = "Save a request before attempting one notification. After uncertain delivery, recover with show, wait or sent; do not send it again under a new ID.",
        after_help = "Example, after both peers are registered:\n  htalk --as alice send bob --message 'Please check this.' --wait 45\nSave id as REQUEST_ID. If reply is present, read reply.body and ack reply.id.\nOtherwise continue with htalk --as alice wait REQUEST_ID.\nPut --db PATH and --as NAME before send, or use HTALK_DB and HTALK_PEER."
    )]
    Send {
        #[arg(value_name = "recipient", help = "Registered recipient peer name.")]
        recipient: String,
        #[arg(
            long,
            value_name = "id",
            help = "Caller-generated UUID, saved before sending. An identical retry returns the saved request without another notification."
        )]
        id: Option<String>,
        #[command(flatten)]
        body: Body,
        #[arg(long, help = "Save for inbox polling without notifying the client.")]
        no_notify: bool,
        #[arg(
            long,
            value_name = "SECONDS",
            default_value = "0",
            allow_negative_numbers = true,
            help = "Wait 0–45 seconds for an answer (default: 0). An answer recorded by this wait before the final notification check skips that notice. It does not resend."
        )]
        wait: f64,
    },
    #[command(
        about = "Save an answer to an exact request.",
        long_about = "Answer the exact incoming request. An identical retry returns the saved answer without another notification.",
        after_help = "Example: htalk --as bob reply REQUEST_ID --message 'Checked.'\nREQUEST_ID is the incoming question's id from inbox or show.\nThe saved answer has its own id; the original sender acknowledges that answer."
    )]
    Reply {
        #[arg(
            value_name = "message_id",
            help = "Incoming request UUID from inbox or show."
        )]
        message_id: String,
        #[command(flatten)]
        body: Body,
        #[arg(long, help = "Save for inbox polling without notifying the client.")]
        no_notify: bool,
    },
    #[command(
        about = "Wait for an answer to a saved request.",
        long_about = "Poll a saved outgoing request for its answer. Record the answer before returning it; a notification that sees this record at its final check is skipped. An already accepted notice may still arrive. Safe to resume after timeout or interruption; does not resend or acknowledge.",
        after_help = "Example: htalk --as alice wait REQUEST_ID --seconds 45\nOn timeout, use this same request ID again. If reply is present, read\nreply.body, then run htalk --as alice ack REPLY_ID using reply.id."
    )]
    Wait {
        #[arg(
            value_name = "message_id",
            help = "Outgoing request UUID from send, sent or show."
        )]
        message_id: String,
        #[arg(
            long,
            value_name = "seconds",
            default_value = "45",
            allow_negative_numbers = true,
            help = "Wait 0–45 seconds; 0 checks once (default: 45)."
        )]
        seconds: f64,
    },
    #[command(
        about = "Read a saved message and its correlated answer without acknowledging.",
        long_about = "Read a saved message and its correlated answer without acknowledging.",
        after_help = "Example: htalk --as alice show MESSAGE_ID\nOnly the sender or recipient can show a message. ack_at is its read mark;\nreply is the correlated answer, with its own id and ack_at."
    )]
    Show {
        #[arg(
            value_name = "message_id",
            help = "Message UUID from inbox, sent or another command's result."
        )]
        message_id: String,
    },
    #[command(
        about = "Record that you read an incoming message and remove its pending Codex notice when possible. A question stays open until answered.",
        long_about = "Record that you read an incoming message and remove its pending Codex notice when possible. A question stays open until answered.",
        after_help = "Example: htalk --as alice ack REPLY_ID\nFor an answer returned by wait, use reply.id, not the outgoing request's id.\nRepeated ack is safe. Read notification_cleanup separately: ack may succeed\nwhile cleanup fails. Use recovery.retry_notification_cleanup when returned.\nClaude/OpenCode do not support withdrawing an already queued notice."
    )]
    Ack {
        #[arg(
            value_name = "message_id",
            help = "Message UUID from inbox, sent or another command's result."
        )]
        message_id: String,
    },
    #[command(
        about = "List incoming work that remains open.",
        long_about = "Read incoming unanswered questions and unacknowledged answers, oldest first. Reading changes no acknowledgments.",
        after_help = "Example: htalk --as bob inbox\nRead messages[].body, then ack that message's id. Reply to a question\nusing the same id; acknowledging alone leaves the question open.\nWhen omitted is above 0, run next_page for newer messages."
    )]
    Inbox {
        #[arg(
            long,
            value_name = "N",
            default_value = "20",
            value_parser = integer_argument,
            allow_negative_numbers = true,
            help = "Return at most N messages, 1–500 (default: 20)."
        )]
        limit: String,
        #[arg(
            long,
            value_name = "SEQ",
            value_parser = integer_argument,
            allow_negative_numbers = true,
            help = "Continue with messages newer than this seq; next_page supplies it."
        )]
        after_seq: Option<String>,
    },
    #[command(
        about = "Stream incoming message notices as JSON lines until interrupted.",
        long_about = "Emit ready, then a message event for each open inbox item. Polls locally without model calls. Reading changes no acknowledgments or delivery receipts.",
        after_help = "Example: htalk --as bob watch\nUse for a harness extension that queues notices into its current session.\nEach event contains an id and notification text, not the peer's message body.\nOpen messages appear once per watcher; restarting replays unfinished work.\nAlways show the current message before acting. Run one receiver per peer."
    )]
    Watch,
    #[command(
        about = "List outgoing messages and recover their IDs.",
        long_about = "Recover outgoing IDs after interruption, including messages with uncertain notifications, newest first. Does not resend.",
        after_help = "Example: htalk --as alice sent\nUse a saved request's id with show or wait. Inspect reply for its answer.\nTexts are summarized as body_bytes and body_preview, the first nonblank line\nup to 120 characters; show or --bodies returns them in full.\nWhen omitted is above 0, run next_page for older messages.\nA saved message with an uncertain notification must not be resent."
    )]
    Sent {
        #[arg(
            long,
            value_name = "N",
            default_value = "20",
            value_parser = integer_argument,
            allow_negative_numbers = true,
            help = "Return at most N messages, 1–500 (default: 20)."
        )]
        limit: String,
        #[arg(
            long,
            value_name = "SEQ",
            value_parser = integer_argument,
            allow_negative_numbers = true,
            help = "Continue with messages older than this seq; next_page supplies it."
        )]
        before_seq: Option<String>,
        #[arg(long, help = "Return full message and answer texts.")]
        bodies: bool,
    },
}

// The text of a `send` or a `reply`: given in the command or read from a file, never both.
#[derive(Args)]
#[group(id = "exclusive_0", required = true, multiple = false)]
pub(crate) struct Body {
    #[arg(
        long,
        value_name = "message",
        help = "Nonblank message body, at most 32,000 UTF-8 bytes."
    )]
    pub message: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the same message body from a local text file."
    )]
    pub message_file: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum PeerCommand {
    #[command(
        about = "Register an immutable native or pull peer.",
        long_about = "Save a peer name and native session address, or select --delivery pull for inbox polling without an address. Existing names cannot be reassigned. Does not notify or launch a client.",
        after_help = "Examples, with the exact session ID and workspace from discovery:\n  htalk peer add alice --harness codex --session SESSION_UUID --workspace /project\n  htalk peer add bob --harness claude --session SESSION_UUID --workspace /project\n  htalk peer add muse --harness opencode --session ses_ID --workspace /project --url http://127.0.0.1:4096\n  htalk peer add helper --harness generic --delivery pull\n\nRegister both peers in the same database. Then run htalk peer check NAME."
    )]
    Add(PeerAdd),
    #[command(
        about = "List registered peer addresses.",
        long_about = "Read registered peer names and their immutable session addresses. Retired peers are hidden unless --all is given.",
        after_help = "Example: htalk --db /shared/mail.sqlite3 peer list\nThese are saved addresses; use peer check NAME to inspect a recipient now.\nretired_hidden counts the retired peers left out."
    )]
    List {
        #[arg(long, help = "Include retired peers; their retired_at is set.")]
        all: bool,
    },
    #[command(
        about = "Find native session addresses.",
        long_about = "Find native session addresses without registering or messaging them. Source statuses describe discovery coverage; an empty result does not prove that no client is running.",
        after_help = "Examples:\n  htalk peer discover --harness claude --workspace /project\n  htalk peer discover --harness opencode --opencode-url http://127.0.0.1:4096\nUse sessions[].session_id and workspace with peer add; inspect sources for gaps."
    )]
    Discover {
        #[arg(
            long,
            value_name = "harness",
            value_parser = ["codex", "claude", "opencode"],
            help = "Inspect only this client (default: all three)."
        )]
        harness: Option<String>,
        #[arg(
            long,
            value_name = "workspace",
            help = "Only return sessions matching this resolved workspace path."
        )]
        workspace: Option<String>,
        #[arg(
            long,
            value_name = "codex_socket",
            help = "Inspect this running app-server socket; repeat for several servers."
        )]
        codex_socket: Option<Vec<String>>,
        #[arg(
            long,
            value_name = "opencode_url",
            help = "Inspect this local OpenCode server; repeat for several servers."
        )]
        opencode_url: Option<Vec<String>>,
    },
    #[command(
        about = "Check a registered session's identity without messaging.",
        long_about = "Verify available identity evidence for a registered peer. Run in the scope that will send; unavailable evidence does not prove that the client is offline.",
        after_help = "Example: htalk peer check bob\nRun this before send. A successful check does not prove message receipt."
    )]
    Check {
        #[arg(value_name = "name", help = "Registered peer name.")]
        name: String,
    },
    #[command(
        about = "Refuse new requests to or from a peer that is no longer used.",
        long_about = "Mark a registered peer retired. New requests to or from it are refused, and notices to it are skipped. Replies to saved requests, inbox, show, wait and ack keep working. The name and session stay bound.",
        after_help = "Example: htalk peer retire bob\npeer list then hides bob; peer list --all shows its retired_at. Repeating keeps\nthe first time. Replies to saved requests remain possible."
    )]
    Retire {
        #[arg(value_name = "name", help = "Registered peer name.")]
        name: String,
    },
    #[command(
        about = "Allow new requests to or from a retired peer again.",
        long_about = "Clear a peer's retirement. Notices skipped while it was retired are not sent. Repeating is safe.",
        after_help = "Example: htalk peer restore bob"
    )]
    Restore {
        #[arg(value_name = "name", help = "Registered peer name.")]
        name: String,
    },
}

#[derive(Args)]
pub(crate) struct PeerAdd {
    #[arg(
        value_name = "name",
        help = "Local name: 1–64 lowercase letters, digits, _ or -; start with a letter or digit."
    )]
    pub name: String,
    #[arg(
        long,
        value_name = "harness",
        help = "Harness label. Native: codex, claude or opencode. Pull: any lowercase peer-style ID."
    )]
    pub harness: String,
    #[arg(
        long,
        value_name = "delivery",
        default_value = "native",
        value_parser = ["native", "pull"],
        help = "Native client notification, or inbox polling without a native session."
    )]
    pub delivery: String,
    #[arg(
        long,
        value_name = "session",
        required_if_eq("delivery", "native"),
        help = "Exact ID from discovery: Codex/Claude UUID or OpenCode ses... ID."
    )]
    pub session: Option<String>,
    #[arg(
        long,
        value_name = "workspace",
        required_if_eq("delivery", "native"),
        help = "Session workspace path; must match after resolving paths."
    )]
    pub workspace: Option<String>,
    #[arg(
        long,
        value_name = "socket",
        help = "Codex only: explicit standalone app-server Unix socket. Omit to use native codex queue."
    )]
    pub socket: Option<String>,
    #[arg(
        long,
        value_name = "url",
        help = "OpenCode only: loopback server URL (default: http://127.0.0.1:4096)."
    )]
    pub url: Option<String>,
}

// What the MCP tool takes. A doc comment here would become part of its input schema.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Arguments {
    /// CLI arguments, e.g. ["inbox"], ["show","ID"], ["reply","ID","--message","answer"].
    pub args: Vec<String>,
    // Gemini CLI forwards its client-side ordering hint after scheduling the call.
    #[serde(default, rename = "wait_for_previous")]
    #[schemars(skip)]
    _wait_for_previous: bool,
    // A published endpoint is narrowed to the profiles its caller selected.
    #[cfg(feature = "catalog")]
    #[serde(default, rename = "_catalog_binding")]
    #[schemars(skip)]
    pub scope: Option<crate::catalog::Binding>,
}

impl Arguments {
    /// The arguments for an endpoint that takes no scope, or its refusal.
    pub(crate) fn unscoped(self) -> Result<Vec<String>, &'static str> {
        #[cfg(feature = "catalog")]
        if self.scope.is_some() {
            return Err(crate::catalog::NO_SCOPE);
        }
        Ok(self.args)
    }
}

/// What the MCP tool does with a command.
pub(crate) enum OverMcp {
    Run,
    /// A `send` there must name its message, so a lost answer can be looked up.
    NeedsId,
    /// Outside the tool, with the name of what the call asked for.
    Refuse(&'static str),
}

impl Mailbox {
    /// The MCP tool runs the reading and messaging commands and `peer list` and `peer check`.
    /// It never reads a file, and a command added to the table is refused until it is named here.
    pub(crate) fn over_mcp(&self) -> OverMcp {
        match self {
            Self::Send { body, .. } | Self::Reply { body, .. } if body.message_file.is_some() => {
                OverMcp::Refuse("--message-file")
            }
            Self::Send { id: None, .. } => OverMcp::NeedsId,
            Self::Send { .. }
            | Self::Reply { .. }
            | Self::Wait { .. }
            | Self::Show { .. }
            | Self::Ack { .. }
            | Self::Inbox { .. }
            | Self::Sent { .. }
            | Self::Peer(PeerCommand::List { .. } | PeerCommand::Check { .. }) => OverMcp::Run,
            Self::Peer(PeerCommand::Add(_)) => OverMcp::Refuse("peer add"),
            Self::Peer(PeerCommand::Discover { .. }) => OverMcp::Refuse("peer discover"),
            Self::Peer(PeerCommand::Retire { .. }) => OverMcp::Refuse("peer retire"),
            Self::Peer(PeerCommand::Restore { .. }) => OverMcp::Refuse("peer restore"),
            Self::Migrate => OverMcp::Refuse("migrate"),
            Self::Watch => OverMcp::Refuse("watch"),
        }
    }

    /// The message a command names: its `message_id`, or the `--id` of a `send`.
    pub(crate) fn message_id(&self) -> Option<&str> {
        match self {
            Self::Send { id, .. } => id.as_deref(),
            Self::Reply { message_id, .. }
            | Self::Wait { message_id, .. }
            | Self::Show { message_id }
            | Self::Ack { message_id } => Some(message_id),
            Self::Peer(_)
            | Self::Migrate
            | Self::Inbox { .. }
            | Self::Watch
            | Self::Sent { .. } => None,
        }
    }
}
