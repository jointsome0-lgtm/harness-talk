# Receive remote mail in an existing Codex session

Keep the mailbox on the device that remains available. Requests can then be
saved while the worker's connection is down. `htalk mcp --connect` restores
access on the next tool call, and `htalk receive` forwards pending notices to
one existing Codex CLI session after its watch connection returns.

These commands require htalk 0.9.0 on the worker. The fixed mailbox endpoints
can use 0.8.1. They share one SQLite mailbox; they do not synchronize databases.
The worker must already be running and its device awake. This route was
checked on Linux with Codex CLI 0.157.1; see the
[LAN reconnection check](../docs/checks/2026-09-27-ssh-reconnect.md).

## Configure the two endpoints

Register a fresh pull peer for the worker on the mailbox host. Give each
worker its own mailbox identity. Use the [restricted SSH setup](ssh.md) to
provide two fixed commands, with a separate restricted key for each:

```sh
/absolute/htalk --db /private/mail.sqlite3 --as laptop-worker mcp
/absolute/htalk --db /private/mail.sqlite3 --as laptop-worker watch
```

The worker's MCP tool and receiver must reach the same database and peer.
Neither command should accept a database, identity or shell command from
incoming mail. SSH authenticates the endpoint. A server advertising the
`harness-talk` product name is not proof of its identity.

Configure the worker's local MCP executable as `htalk`, with arguments
`mcp --connect -- ssh ...`. Preserve all the host-key, key-selection and
connection limits from the [SSH example](ssh.md#connect-the-remote-agents-mcp-client).
Do not put `--db` or `--as` before `mcp --connect`: the remote endpoint fixes
both. Local `HTALK_DB` and `HTALK_PEER` do not select the remote identity.
The connector is an executable plus arguments, run directly without a shell.

The local MCP process survives a failed connection. Each tool call starts a
new connector, initializes the remote MCP server and sends one `tools/call`.
It never automatically resends a call. A lost reply can hide a committed write;
inspect its saved message ID before repeating a command or the work itself.
Calls have a 120-second deadline and honor client cancellation. A later call
can connect again without restarting the harness.

## Start the receiver

Start the ordinary Codex TUI with the remote MCP entry and its normal model
and permission settings. Give it the owner's task scope and wait for its
initial turn to finish. Discover the exact CLI session UUID and workspace with
`htalk peer discover --harness codex`. Do not select an approval helper or a
different conversation in the same folder.

Run the following under the same Codex account and environment, with the
ordinary `codex` CLI on PATH. Here `/private/watch-connector` is an
owner-prepared executable containing the fixed SSH watch connection above.
It accepts no arguments from mail and writes only the watch JSON stream to
stdout. It must stay open while the connection is healthy.

```sh
htalk receive --peer laptop-worker --session EXACT_SESSION_UUID \
  --workspace /absolute/project --state /private/receiver-state \
  -- /private/watch-connector
```

The receiver creates a private state directory if it is absent. Keep that
directory for every restart of this receiver. It locks the directory, checks
the saved Codex identity and binds state to the peer, session, workspace and
connector arguments. Changing that binding is rejected.

Since 0.9.2, a second receiver targeting the same saved Codex session is also
rejected when it uses a different state directory. Ownership is locked under
`htalk-receivers/SESSION_UUID.lock` beside the resolved Codex `state_5.sqlite`.
Different Codex homes pointing to that same store share the lock. It is held
until any in-flight native submission has finished. Keep these private lock
files in place; do not unlink them to bypass an active receiver.

Stop receivers before changing `sqlite_home` or moving the selected Codex
metadata store. Since 0.9.3, a receiver refuses a new notice with
`codex_store_changed` if the selected store differs from its locked startup
store. It also pins each native `codex queue` command to that captured store,
so a config edit between the check and command startup cannot redirect the
write. An already running submission finishes against its original store.
A file symlink targeting a basename other than `state_5.sqlite` is refused with
`codex_store_name_unsupported`, since `sqlite_home` cannot select that file.
Restart with the existing receipt directory after the intended relocation;
the guard does not synchronize separately copied stores or their queues.

This is local process ownership, not a cross-host lease or a task claim.
Shared network filesystems and independently copied Codex stores are outside
this guarantee. Processes using the same OS account can remove lock files;
this is coordination between cooperating receivers, not account isolation.
Stop older receivers before updating, since they do not acquire this lock.
After stopping a receiver, reuse its original state directory: the session
lock does not move receipts into a new directory or resolve uncertain work.

When the watch connection closes, the receiver reconnects after 1, 2, 4, 8,
16 and then at most every 30 seconds. This reconnects a stream; it does not
call a model on a timer. Only a validated message ID becomes a native Codex
notification. Remote notification prose and remote database paths are not
injected into the worker. The worker reads the message through its MCP tool.

A watch event can become obsolete while buffered in the connection. Use the
current-mail check below to skip completed messages before native submission.
The agent must still inspect `show` before acting: a request with a saved reply
or an ACKed answer needs no further processing, while an ACKed unanswered
request remains open. Mail can change after any read. This check and local
ownership do not provide exactly-once external work.

Before native submission, the receiver saves the message as `pending` and
syncs the file. After a verified queue receipt, it saves that receipt and
clears `pending`. Previously submitted IDs are skipped when watch reconnects
or the receiver restarts. Receipts prove notification submission, not reading,
ACK or task completion. The receiver never ACKs or replies on the agent's behalf.

If native submission has an uncertain result, the receiver stops. Restarting
with unresolved `pending` also stops before contacting Codex. Preserve the
state and inspect the exact message, native queue and session history. Do not
remove the state directory, invent a new message ID or launch another worker
to bypass uncertainty. A worker that received a notice but failed before
replying needs explicit recovery; reconnecting is not permission to rerun it.

A native identity check reads saved CLI metadata. It does not prove a live
TUI or queue consumption. Verify a bounded task and its correlated answer
independently. This receiver neither starts stopped clients nor resumes an
interrupted task automatically.

## Check current mail

With worker version 0.9.3, add an absolute executable path to the fixed MCP
connector already used by the worker. For example, `/private/mcp-connector`
can execute the owner's pinned SSH command or connect to the private MCP
socket. It receives no command-line arguments from the receiver. It must speak
MCP on stdin/stdout and reach the same mailbox and peer as the watch connector.
Reuse the same wrapper in the worker's `htalk mcp --connect -- ...` setup.

```sh
htalk receive --peer laptop-worker --session EXACT_SESSION_UUID \
  --workspace /absolute/project --state /private/receiver-state \
  --mcp-command /private/mcp-connector -- /private/watch-connector
```

For every new watch ID, the receiver calls `show` through that authenticated
endpoint before saving any native submission intent. It validates the returned
ID, sequence and recipient. A request with a saved reply or an ACKed answer
emits `skipped`; an unanswered request still wakes even after ACK. No ACK or
reply is written by the receiver, and a skip is not a native queue receipt.

If the read connection fails, it emits `check_unavailable` and reconnects the
watch stream with the existing backoff. It does not wake without a successful
check. A rejected, malformed or mismatched mailbox result stops for inspection.
Cancellation waits for checker cleanup as well as any native submission.

This option can be added to an existing receiver state once, provided no
notification outcome is unresolved. Old receipts are kept. The executable path
then becomes part of the saved binding; changing or removing it is refused.
Preserve the original connector and state for recovery. Old receiver commands
without this option remain compatible but can still wake on stale events.
The mailbox endpoint needs no new protocol or schema; its existing MCP `show`
operation supplies the read. Authentication remains the connector's job.
The receiver's `ready` event reports whether `mailbox_check` is enabled.

## When only the laptop accepts SSH

The mailbox PC can initiate a reverse SSH tunnel to private Unix sockets on
the laptop. On the PC, a socket supervisor runs a fixed `mcp` or `watch`
command per accepted connection. The laptop connects with, for example,
`socat - UNIX-CONNECT:/private/htalk/mcp.sock` for MCP and the corresponding
watch socket for the receiver.

Keep both socket directories private and bind each endpoint to the chosen
mailbox and peer. The connecting SSH account needs Unix-socket forwarding;
a key configured with `restrict` alone does not permit it. The owner controls
this tunnel, and its private key stays on the PC. The worker needs access to
its private sockets, not the PC's unrestricted SSH credentials. The tested
route opened no TCP listener on the PC and changed no firewall settings.

A stopped reverse tunnel can leave socket files on the SSH server. The client's
`StreamLocalBindUnlink` setting does not remove those server-side files. Before
recreating this fixture, the controller verified that both old tunnel processes
had exited and each socket refused connections, then removed only those two
owned socket paths. A production tunnel supervisor needs equivalent lifecycle
handling; the htalk receiver does not manage SSH listeners.

The [Linux user-service setup](ssh-service.md) provides this separate transport
supervisor and a helper that preserves live or uncertain socket paths.

Stopping the tunnel removes connectivity, not saved mail. Stop the receiver
and close active connections when retiring the worker. Preserve its receipt
directory. Bluetooth, WAN operation and native Windows/macOS remain separate
checks; an SSH tunnel over the LAN does not establish those capabilities.
