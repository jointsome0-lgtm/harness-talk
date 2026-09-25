# Client adapters

The [Pi, Hermes, OpenClaw and Agent Zero source integrations](../integrations/README.md) use a shared
`htalk watch` receiver. On 2026-09-25, Pi 0.87.1 in RPC mode and Hermes 0.21.5 in
classic CLI mode completed a Linux exchange with GPT-6 Luna through OpenRouter
Flex. An incoming notice woke Pi; Pi asked Hermes for a calculation, Hermes
answered through its `htalk` tool, and Pi returned the independently checked
result to the original sender. All four message links and acknowledgments were
verified in an isolated mailbox. Restarting the receivers recovered the saved
requests without sending new copies. This source build is not yet a release;
Hermes gateway and modern TUI remain unsupported.

The reverse Hermes → Pi exchange also completed after resuming Hermes's saved
session. The first attempt exhausted its verification budget after reading and
acknowledging Pi's answer. Recovery exposed the htalk tool directly, prohibited
new sends, and answered the original request without repeating the delegation.
All four saved messages and ACKs were checked. This demonstrates recovered
completion, not an uninterrupted first-attempt pass.

OpenClaw 2026.9.6 then completed controller → OpenClaw → Pi → OpenClaw → controller
using its built-in runtime, the same mailbox, and Luna/Flex. The Gateway's
targeted notification wake started the configured main session with periodic
heartbeat and cron disabled. All four messages, reply links, ACKs and the
arithmetic result were checked. There were no model calls before the first
message. After the saved exchange completed, the fixture's ten-request cap
blocked further OpenClaw continuation. A separate model-free check verified
plugin loading, a 25-message backlog, session-scoped tool access and watcher
cleanup.

A fresh Pi → OpenClaw → Pi exchange with direct tool schemas then completed
within both request caps. Four correctly linked messages were acknowledged;
Pi independently checked OpenClaw's answer. OpenClaw recorded native session
status `done` and trajectory outcome `success`, with no guard rejection. The
temporary runner incorrectly expected `completed`, so it reported a timeout;
the saved mailbox and native trajectory established the successful result
without rerunning the exchange. These checks do not cover other OpenClaw
runtimes or operating systems.

Agent Zero revision `e3051fb584b1a36be2b0a0c90606f1c2c2d356ec` was checked
on Python 3.12 and Linux with its actual context registry, plugin loader,
extension hooks and tool class. The model-free checks exercised a paused/busy
backlog, user-queue priority, context-scoped tool access, CLI inbox/show/ack/reply,
context replacement, plugin reload/deletion and watcher cleanup. A separate
ordinary framework turn used a local canned model response: an incoming notice
woke the agent, exposed the htalk tool prompt/schema and ended through the native
response tool. Idle receivers made no model calls. A local wire capture checked
Luna/Flex settings for chat, utility and vision; it made no external API request.

The real Agent Zero/Pi model exchange remains pending. These checks do not
establish WebUI/container installation, other operating systems or full startup
with default plugins and embeddings. The pinned Agent Zero API also has a
narrow concurrent pause race, documented in [setup](../integrations/README.md#agent-zero).

Checked on 2026-09-23 with the htalk `0.6.1` x86-64 publication wheel from commit `6b6b9efef90455ecff640049f38d4e14f15ace37`, SHA-256 `89befb955c10b8b49796bfcdae3f3b2e03fabc98dd84d192dd2858aeaba84e75`. Its published PyPI file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-6-astra`, low) ran in separate Linux tmux terminals. Claude retained manual permissions with the fixture executable allowed; Codex retained `workspace-write`, on-request escalation and automatic approval review, using approved host execution for each htalk command.

The isolated mailbox was created with htalk `0.5.1`, with two native peer registrations and no messages. A normal `peer list --all` using 0.6.1 upgraded schema 2 to 3. The one private backup matched the legacy SQLite dump, and the original peer fields were preserved. A pull participant was then registered with 0.6.1.

Each native client initiated a request and consumed the other's correlated reply. Codex → pull and pull → Claude also completed. All eight messages were shown and acknowledged separately, with each request acknowledged before its reply. Six native notices were submitted and consumed once; two pull messages made no notification attempt. All three final inboxes were empty, the clients exited and the peers were retired. No message or notification was repeated. Codex cleanup was `absent`, Claude cleanup `unsupported`, and pull cleanup `skipped/pull_only`.

A separate [OpenCode 1.18.31 check](opencode.md) used the same wheel and completed both initiating directions with a pull participant. These are Linux observations; direct OpenCode ↔ Codex/Claude and cross-device exchanges remain unchecked.

Checked on 2026-09-23 with the htalk `0.6.0` x86-64 publication wheel from commit `9963bb4dee3eeb1a6b63ea64978b34dd1977e109`, SHA-256 `c70f8990a8d66cecad7f96747f6f6dacaaa9721f5532ab8725ef6c42579ad23b`. The wheel was installed in a fresh environment before PyPI approval; the published file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-6-sol`, medium) ran in separate Linux tmux terminals with an isolated schema-3 mailbox. Claude used manual permissions with the test executable allowed. Codex retained `workspace-write`, on-request escalation and automatic approval review; each htalk command used approved host execution.

Codex and Claude each initiated a request and consumed the other's correlated reply. A Codex request to a pull participant and a pull request to Claude also completed with correlated replies. All eight messages were shown and acknowledged in separate commands, with acknowledgment preceding each reply. Six native notices were submitted and consumed once; the two pull messages made no notification attempt. No send, reply or notice was repeated, no answer was polled through `wait`, and all three final inboxes were empty. The native sessions were closed and their test peers retired. Codex cleanup was `absent`, Claude cleanup was `unsupported`, and pull cleanup was `skipped/pull_only`; acknowledgment does not imply notification deletion.

A separate [OpenCode 1.18.31 check](opencode.md) used the same publication wheel and completed both initiating directions with a pull participant. These observations cover all three native clients with the common mailbox on Linux; direct OpenCode ↔ Codex/Claude and cross-device exchanges were not tested.

Checked on 2026-09-22 UTC with the htalk 0.5.1 x86-64 release wheel from commit `ff28ef3c55b952d7900ee076b37a8a0bf4086ba5`, SHA-256 `be86ab5aeed823c648912949b397776ee71b8206384e84c844cbab6b234ce502`. The publication workflow's wheel was installed in a fresh environment before PyPI approval; the published PyPI file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-5.6-luna`, low) TUI sessions ran in separate tmux terminals on Linux with an isolated synthetic database. Claude used manual permissions with the test executable allowed. Codex retained `workspace-write`, on-request escalation and automatic approval review; every htalk command used approved host execution.

One request and its correlated reply completed in each direction. All four notifications were submitted, consumed through native notices, read and acknowledged. Neither recipient needed controller assistance to retrieve an answer. The initial Codex command used a nonexistent `request` subcommand and was rejected before storage; after verifying that no message existed, the controller supplied the correct `send` syntax. In the reverse exchange Codex read the request, replied, then acknowledged it, departing from the planned acknowledgment order. This verifies the transports with those setup and ordering deviations; it was not a fully unassisted execution of the test protocol.

Both Codex acknowledgments reported `notification_cleanup: absent`, because their notices had already been consumed. Both Claude acknowledgments reported `unsupported` with `client_has_no_notification_removal`. No notification or cleanup was retried, and no answer was polled through `wait`. OpenCode was not installed for this check; its earlier observation remains in [OpenCode sessions](opencode.md).

Checked on 2026-09-21 with the htalk 0.5.0 x86-64 release wheel from commit `fb6f93ea85372484749cc86ab062d8e511107923`, SHA-256 `46c4db64751ba6b56b5c13d83a8b74e1eb818745712af65fed11beca82448e87`. The wheel was downloaded from the publication workflow and installed in a fresh environment before PyPI approval; the published file has the same hash. Ordinary Claude Code `2.1.278` (Opus 5, medium) and Codex CLI `0.154.0` (`gpt-5.6-luna`, low) TUI sessions ran in separate tmux terminals on Linux, using an isolated synthetic database. Claude used manual permission mode with the test htalk command allowed. Codex used a `workspace-write` sandbox, on-request escalation and automatic approval review.

A Codex-initiated request and its Claude answer were submitted, consumed through native notices, read and acknowledged; the answer's `in_reply_to` matched the request. In the first reverse case, Codex omitted the requested host escalation: its answer was saved, but the Claude notification was `not_submitted` with `recipient_unavailable`. Claude recovered that answer by an explicitly instructed `show` and `ack`, without a resend. In one distinct corrective case, the new request's notice arrived before Codex processed its control instructions. Codex read and acknowledged the request, then stopped at the unexpected body. After an explicit controller continuation, it replied through the approved host scope; Claude consumed the native answer notice, read the correlated answer and acknowledged it. This corrective case verifies the transports with host access, but required controller intervention.

Across those three requests and three replies, all six messages were acknowledged; five notifications were submitted and one was not submitted. Codex's three acknowledgments saved the read mark but reported `notification_cleanup: unavailable` with `codex_rpc_closed` inside its sandbox. Claude's three reported `unsupported` with `client_has_no_notification_removal`. Cleanup was not retried, and no notification was replayed. OpenCode was not installed for this release check; its earlier observation remains in [OpenCode sessions](opencode.md).

Verified on 2026-09-08 with htalk 0.1.1: ordinary Codex `0.153.4` and Claude Code `2.1.263` TUI clients in tmux on Linux consumed native notifications without manual recipient advancement. Both initiating directions received, read and acknowledged the exact correlated answer. These are compatibility observations, not a guarantee for other builds.

In the Claude-initiated exchange, Codex could not discover Claude from its ordinary tool execution scope: the reply was saved, but its notification was `not_submitted`. Claude recovered and acknowledged the answer through `--wait`. In the reverse exchange, Codex used its standard approved host command scope for the read-only `peer check` and a single send. Idle Claude consumed the notification and replied; Codex read and acknowledged the answer, then later consumed its queued notice. Three of the four notifications were submitted; none was replayed.

Checked on 2026-09-17 with the htalk 0.4.0 release wheel built from commit `fae93218bc5fcdb66f8e67ca0d1be2eb4117814e`, with SHA-256 `254b3b7e67a4f6d5e9e46ea57d0817872c34fca755946cfd169749e05f389db2`; PyPI serves the same file. Claude Code was checked before the release was published, and Codex after. Two ordinary Claude Code `2.1.274` TUI sessions ran in tmux on Linux, in manual permission mode with `htalk` commands allowed. One sent a request. The other consumed its notice, then read, acknowledged and answered it. The first consumed the answer's notice, then read and acknowledged the answer. Both notifications were `submitted` with `claude_socket_bytes_written`. The answer's `in_reply_to` matched the request. Both acknowledgments reported `notification_cleanup` as `unsupported` with `client_has_no_notification_removal`. Claude Code showed each notice as a message from another Claude session, followed by its own guidance for peer messages.

An ordinary Codex `0.154.0` TUI then exchanged one request and its answer in each direction with a Claude Code session set up the same way. Codex used `gpt-5.6-luna` at low reasoning effort and the user's permission settings: a `workspace-write` sandbox and on-request escalation with automatic approval review. `--add-dir` made the database directory writable. A Codex TUI started without a prompt had no saved thread, so this one answered one prompt before registration. Each recipient was idle when its notice arrived; it consumed the notice and read the message. All four notifications were `submitted`: `codex_cli_queued:QUEUE_UUID` toward Codex and `claude_socket_bytes_written` toward Claude Code. Both answers' `in_reply_to` matched their requests. As instructed, Codex requested escalation for `peer check`, `send` and `reply`, which reach the Claude Code socket; each was approved without a prompt. Codex ran `show` and `ack` in the sandbox. Both of its acknowledgments saved the read mark and reported `notification_cleanup` as `unavailable` with `codex_rpc_closed`. Repeated outside the sandbox, cleanup reported `absent`, because Codex had already consumed both notices. The Claude Code acknowledgments reported `unsupported`, as above.

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

Ordinary local TUI sessions use `codex queue --thread EXACT_UUID --message TEXT`. `htalk` passes no model, sandbox, approval, profile, or configuration overrides and never types into tmux. Both Codex routes reread the saved message state after their identity check and before queueing; an acknowledged message, or an answer the requester's `wait` has already returned, is not queued.

Before invoking that command, `htalk` opens `state_5.sqlite` read-only and selects only the exact session's `id`, `cwd`, `archived`, and `source`. It requires the registered UUID/workspace, an unarchived row and `source=cli`. The metadata directory comes from the user-level `sqlite_home` setting in `$CODEX_HOME/config.toml`, then `CODEX_SQLITE_HOME`, then `$CODEX_HOME`, defaulting to `~/.codex`. Missing metadata, a different workspace, an archived session, or an unexpected schema prevents submission.

This is a version-specific saved-identity check, not a public stable lookup API or proof of an active TUI. `peer check` reports the metadata file and `runtime_status: unknown`. The adapter does not inspect process environments or acquire native writer locks. It does not reproduce Codex's complete layered configuration loader; system/project/profile-only metadata relocations are outside this tested lookup. Use the same local client metadata as the receiving TUI and inspect `peer check` before sending. The CLI itself receives the exact UUID, never a display name.

A successful native receipt must match both `Queued message QUEUE_UUID for thread SESSION_UUID.` and exit code 0. `QUEUE_UUID` is the client queue identifier; the htalk message UUID remains in the durable database and notification text. A timeout, nonzero exit or unfamiliar receipt after the process starts is `submission_unknown`, with no retry. A saved identity cannot guarantee the client will consume the queue; only a reply or explicit acknowledgment establishes later progress.

The native route is grounded in the installed command help, the actual queue receipt, and the official [session queue implementation](https://github.com/openai/codex/blob/main/codex-rs/tui/src/session_queue_commands.rs). That implementation passes UUID targets directly to the queue API and may use an embedded app server internally. `htalk` does not require or manage a persistent app-server process for ordinary TUI sessions.

For previously configured standalone app-server sessions, the explicit `--socket` mode remains available. It verifies Unix socket ownership and exact live UUID/workspace with status `idle` or `active`, then calls `thread/queue/add` once on the same connection. This build requires `experimentalApi: true` and rejects WebSocket compression, so compression is disabled. No explicit-socket failure falls back to the native CLI, which could address a different client environment. This mode does not force queue consumption or start a thread.

The official [App Server documentation](https://learn.chatgpt.com/docs/app-server) describes the Unix WebSocket and metadata-only `thread/read`. The installed CLI's generated schema supplies the experimental queue shape. The [CLI reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli) describes queued input. The Rust executable uses tungstenite for this standalone mode; the pip package has no Python runtime dependencies. Only the inspected client version is claimed as tested.

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

A command run by Claude Code can omit `--as`: htalk then locates the same metadata file through the command's `CLAUDE_PID`, without running `claude`, and verifies process ancestry in `/proc`. See [peer selection](reference.md#database-and-peers) for the checks and their limits. In the 2026-09-17 checks with Claude Code `2.1.274`, every message command run by Claude Code omitted `--as` and reported `actor_source: native_session`. A detached tmux window given one session's `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PID` was refused with `claude_pid_not_an_ancestor`. After `/clear`, that session had a new ID, and htalk selected no peer for it. `claude --resume` with the original ID kept that ID, and the session's peer was selected again.

The frame contains `type: user`, session and message UUIDs, an honest `htalk:PEER` sender, `priority: next`, and a message pointing to the inbox. It does not assert permission classes or impersonate a native Claude peer. A successful socket write proves only that the bytes were written. The adapter does not claim the model saw them.

This transport is an observed local client interface. It is not documented here as a stable public Claude API. Native inbound controls and filesystem permissions remain in force. If discovery fails, the message remains available in the shared inbox with `not_submitted`. `recipient_unavailable` does not establish that Claude is offline: discovery depends on the invoking command's execution scope. Check the peer in the intended send scope first; a known live session may require normal client permission approval for those specific commands.

After discovery and socket connection, the adapter rereads the saved message state before writing the frame. If the recipient acknowledged it during preflight, or its `wait` has already returned this answer, no frame is written. This was checked with ordinary Claude Code `2.1.267` on 2026-09-10 using a controlled preflight delay until after acknowledgment and the current turn's final response. The message remained acknowledged, no notification entered the native queue, and no extra turn appeared during the following minute.

The inspected `2.1.267` ordinary-session socket handler has no message-cancellation action. The SDK's separate `cancel_async_message` protocol does not establish support through this socket. An already accepted Claude notice cannot be withdrawn by htalk; in an earlier native case it arrived after acknowledgment within the current turn. The final database check narrows the race but cannot eliminate delivery that starts after the check and before acknowledgment.

## OpenCode

OpenCode uses an explicit loopback HTTP server and opaque session IDs. The server confirms the session and workspace before one `prompt_async` POST. Read [OpenCode setup and compatibility](opencode.md) for authentication, discovery coverage and how an accepted prompt can start a turn in an idle existing session.

## Adding an adapter

Pull peers already expose the shared CLI without a native adapter: `peer add NAME --harness LABEL --delivery pull`. The label describes the caller and does not install, discover or wake a client.

Keep persistence and conversation rules in `Store`. `Peer` stores an open harness label and delivery mode; `notify` converts native addresses to `NativePeer` and dispatches to the built-in `Harness` registry. Registration normalization belongs in `notify::native_peer`, not storage. New native adapters extend that registry, dispatcher and validation; they do not change the mailbox schema. Unknown adapters leave list, inbox, show and ACK usable, and fail notification with `adapter_unavailable`. A notification function takes the registered recipient and saved message, then returns an `Outcome` with `submission` and `detail`. The shared notification dispatcher provides a read-only callback for the final database check. The store claims the one attempt before calling it. A failure after client submission might have begun must return `submission_unknown`; only a failure known to precede transmission may return `not_submitted`. An interruption leaves the claim uncertain. Report fixed codes with `Failure::Coded`; other failures carry only the corresponding exception class name through `Failure::Class`, preserving the diagnostic contract of 0.4.

An adapter must verify available evidence for the exact session address, never broaden delivery to a name match, and never replay an uncertain attempt. Adding a harness also requires registration validation and focused tests. Shared storage, reply correlation, acknowledgments and waiting remain unchanged.

The [Codex RPC transport](../src/codex/rpc.rs) runs tungstenite over a Unix stream with bounded connection and RPC timeouts. Its socket mode accepts Unix sockets only; OpenCode has its separate loopback HTTP transport.
