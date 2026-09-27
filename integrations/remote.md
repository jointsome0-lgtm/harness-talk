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

When the watch connection closes, the receiver reconnects after 1, 2, 4, 8,
16 and then at most every 30 seconds. This reconnects a stream; it does not
call a model on a timer. Only a validated message ID becomes a native Codex
notification. Remote notification prose and remote database paths are not
injected into the worker. The worker reads the message through its MCP tool.

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

Stopping the tunnel removes connectivity, not saved mail. Stop the receiver
and close active connections when retiring the worker. Preserve its receipt
directory. Bluetooth, WAN operation and native Windows/macOS remain separate
checks; an SSH tunnel over the LAN does not establish those capabilities.
