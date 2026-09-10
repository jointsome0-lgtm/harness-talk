# Client adapters

Verified on 2026-09-08 with htalk 0.1.1: ordinary Codex `0.153.4` and Claude Code `2.1.263` TUI clients in tmux on Linux consumed native notifications without manual recipient advancement. Both initiating directions received, read and acknowledged the exact correlated answer. These are compatibility observations, not a guarantee for other builds.

In the Claude-initiated exchange, Codex could not discover Claude from its ordinary tool execution scope: the reply was saved, but its notification was `not_submitted`. Claude recovered and acknowledged the answer through `--wait`. In the reverse exchange, Codex used its standard approved host command scope for the read-only `peer check` and a single send. Idle Claude consumed the notification and replied; Codex read and acknowledged the answer, then later consumed its queued notice. Three of the four notifications were submitted; none was replayed.

## Discovery results

`peer discover` returns `sessions` with exact IDs, workspaces, runtime evidence and connection details. `sources` reports `ok`, `partial` or `unavailable`. An empty result describes only the inspected environment: sandboxes, process namespaces, stopped servers and custom client homes can limit coverage. Addresses are a snapshot; `peer check` and notification preflight verify them again.

| Source | Runtime evidence |
| --- | --- |
| Claude `agents --json` | `running` requires a live native record and matching session, workspace and owned socket. It does not indicate model activity. |
| Codex app-server | `idle`/`active` describes a loaded thread; `systemError` reports a loaded thread with a runtime problem. |
| Codex writer locks | `writer_active` means a native CLI holds its kernel writer lock, not that a model turn or queue consumer is active. |
| OpenCode server | `idle`, `busy` or `retry` is the server's state. An idle session need not have an attached TUI. |
| OpenCode saved metadata | `unknown` only establishes a saved, unarchived session. Supply its running server URL before sending. |

Use `--workspace` or `--harness` to filter. Repeat `--codex-socket` or `--opencode-url` to inspect explicit servers. The defaults and limits for each client follow below.

## Codex

Ordinary local TUI sessions use `codex queue --thread EXACT_UUID --message TEXT`. `htalk` passes no model, sandbox, approval, profile, or configuration overrides and never types into tmux. Both Codex routes reread the saved message state after their identity check and before queueing; an acknowledged message, or an answer its requester is polling for, is not queued.

Before invoking that command, `htalk` opens `state_5.sqlite` read-only and selects only the exact session's `id`, `cwd`, `archived`, and `source`. It requires the registered UUID/workspace, an unarchived row and `source=cli`. The metadata directory comes from the user-level `sqlite_home` setting in `$CODEX_HOME/config.toml`, then `CODEX_SQLITE_HOME`, then `$CODEX_HOME`, defaulting to `~/.codex`. Missing metadata, a different workspace, an archived session, or an unexpected schema prevents submission.

This is a version-specific saved-identity check, not a public stable lookup API or proof of an active TUI. `peer check` reports the metadata file and `runtime_status: unknown`. The adapter does not inspect process environments or acquire native writer locks. It does not reproduce Codex's complete layered configuration loader; system/project/profile-only metadata relocations are outside this tested lookup. Use the same local client metadata as the receiving TUI and inspect `peer check` before sending. The CLI itself receives the exact UUID, never a display name.

A successful native receipt must match both `Queued message QUEUE_UUID for thread SESSION_UUID.` and exit code 0. `QUEUE_UUID` is the client queue identifier; the htalk message UUID remains in the durable database and notification text. A timeout, nonzero exit or unfamiliar receipt after the process starts is `submission_unknown`, with no retry. A saved identity cannot guarantee the client will consume the queue; only a reply or explicit acknowledgment establishes later progress.

The native route is grounded in the installed command help, the actual queue receipt, and the official [session queue implementation](https://github.com/openai/codex/blob/main/codex-rs/tui/src/session_queue_commands.rs). That implementation passes UUID targets directly to the queue API and may use an embedded app server internally. `htalk` does not require or manage a persistent app-server process for ordinary TUI sessions.

For previously configured standalone app-server sessions, the explicit `--socket` mode remains available. It verifies Unix socket ownership and exact live UUID/workspace with status `idle` or `active`, then calls `thread/queue/add` once on the same connection. This build requires `experimentalApi: true` and rejects WebSocket compression, so compression is disabled. No explicit-socket failure falls back to the native CLI, which could address a different client environment. This mode does not force queue consumption or start a thread.

The official [App Server documentation](https://learn.chatgpt.com/docs/app-server) describes the Unix WebSocket and metadata-only `thread/read`. The installed CLI's generated schema supplies the experimental queue shape. The [CLI reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli) describes queued input. The retained standalone mode is why `websockets` remains a runtime dependency. Only the inspected client version is claimed as tested.

### Removing acknowledged notifications

`ack` saves the read mark before calling `thread/queue/delete` with the registered thread UUID and the queue UUID from that message's confirmed submission receipt. It never deletes the whole queue. A notice already consumed by the client cannot be recalled.

For native CLI addresses, removal repeats the saved-identity check and uses a temporary `codex app-server --stdio` process with the user's normal configuration. It initializes the protocol, deletes the one queue item and closes the process. It never starts or resumes a thread or requests a model turn. Each RPC has a 10-second timeout, with bounded process shutdown. No persistent server is installed or managed. Explicit socket addresses use their registered socket and repeat the live identity check, with no native fallback.

If `ack` races an in-flight submission, the sender tries the same idempotent removal after saving the queue receipt. A crash or unconfirmed submission can leave a queued notice without a known queue UUID; htalk does not search for or replay it. See [cleanup results and recovery](reference.md#notifications-and-recovery).

### Discovery

`peer discover` connects to `$CODEX_HOME/app-server-control/app-server-control.sock`, or the explicitly supplied `--codex-socket` paths. It pages through `thread/loaded/list` and reads metadata with `includeTurns: false`. It never resumes or subscribes to a thread. Sessions unloaded during discovery and internal workers that cannot accept direct input are excluded. Each server is bounded to 200 inspected IDs, 20 pagination cursors and a 15-second scan budget, plus any already-running bounded RPC. Truncation or a failed page produces source diagnostics while preserving verified addresses.

On Linux, default discovery also matches exclusive kernel `FLOCK` records in `/proc/locks` to owned UUID files in `$CODEX_HOME/thread-writer-locks/`. Only matching unarchived `source=cli` rows are read from the same SQLite address metadata used before notification. The result is `writer_active`; it does not distinguish an idle client from a running model turn. A lock file without a held kernel lock is ignored. No lock is acquired, released or deleted. Process environments and conversation bodies are not read.

The lock lifecycle follows Codex's [writer ownership implementation](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/writer_lock.rs) and [live recorder lifecycle](https://github.com/openai/codex/blob/main/codex-rs/thread-store/src/local/live_writer.rs). This is a version-specific Linux fallback, verified on ext4. Process namespaces can hide kernel records, and older clients may not use these files. Source results describe the inspected environment, not every client on the host. A socket-backed address takes precedence when both sources find the same thread and workspace.

## Claude Code

The adapter runs `claude agents --json` and selects exactly one live record matching both session UUID and workspace. It reads only that PID's `~/.claude/sessions/PID.json` metadata, verifies the UUID and workspace again, and checks the Unix socket's type and owner. It never reads credentials.

The frame contains `type: user`, session and message UUIDs, an honest `htalk:PEER` sender, `priority: next`, and a message pointing to the inbox. It does not assert permission classes or impersonate a native Claude peer. A successful `sendall` proves only that the bytes were written. The adapter does not claim the model saw them.

This transport is an observed local client interface. It is not documented here as a stable public Claude API. Native inbound controls and filesystem permissions remain in force. If discovery fails, the message remains available in the shared inbox with `not_submitted`. `recipient_unavailable` does not establish that Claude is offline: discovery depends on the invoking command's execution scope. Check the peer in the intended send scope first; a known live session may require normal client permission approval for those specific commands.

After discovery and socket connection, the adapter rereads the saved message state before writing the frame. If the recipient acknowledged it during preflight, or is polling for this answer with `wait`, no frame is written. This was checked with ordinary Claude Code `2.1.267` on 2026-09-10 using a controlled preflight delay until after acknowledgment and the current turn's final response. The message remained acknowledged, no notification entered the native queue, and no extra turn appeared during the following minute.

The inspected `2.1.267` ordinary-session socket handler has no message-cancellation action. The SDK's separate `cancel_async_message` protocol does not establish support through this socket. An already accepted Claude notice cannot be withdrawn by htalk; in an earlier native case it arrived after acknowledgment within the current turn. The final database check narrows the race but cannot eliminate delivery that starts after the check and before acknowledgment.

## OpenCode

OpenCode uses an explicit loopback HTTP server and opaque session IDs. The server confirms the session and workspace before one `prompt_async` POST. Read [OpenCode setup and compatibility](opencode.md) for authentication, discovery coverage and how an accepted prompt can start a turn in an idle existing session.

## Adding an adapter

Keep persistence and conversation rules in `Store`. A notification function takes the registered recipient, saved message and database path, then returns `(submission, detail)`. The store claims the one attempt before calling it. A failure after client submission might have begun must return `submission_unknown`; only a failure known to precede transmission may return `not_submitted`. Exceptions leave the claim uncertain.

An adapter must verify available evidence for the exact session address, never broaden delivery to a name match, and never replay an uncertain attempt. Adding a harness also requires registration validation and focused tests. Shared storage, reply correlation, acknowledgments and waiting remain unchanged.

The Codex WebSocket client uses the library's documented [Unix connection helper](https://websockets.readthedocs.io/en/stable/reference/sync/client.html#websockets.sync.client.unix_connect), with bounded connection and RPC timeouts. Its socket mode accepts Unix sockets only; OpenCode has its separate loopback HTTP transport.
