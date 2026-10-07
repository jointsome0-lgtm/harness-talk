# htalk reference

Start with the [first exchange](../README.md). Commands print JSON; `--help` describes their arguments. [Client adapters](adapters.md) document notification and discovery compatibility.

`htalk mcp` and `htalk catalog connect` run MCP stdio servers instead of
returning a single CLI result. Each exposes one mailbox tool with a fixed
peer identity. See [MCP setup](../integrations/mcp.md) and the
[catalogue guide](catalog.md). Model and provider settings remain in the client.

## Database and peers

Choose one writable directory shared by the participants. SQLite writers need directory access for the journal as well as file access. The path is selected in this order:

1. `--db PATH`
2. `HTALK_DB`
3. `$XDG_DATA_HOME/harness-talk/mail.sqlite3`
4. `~/.local/share/harness-talk/mail.sqlite3`

A checkout is never the default database location. New databases use schema 3. Discovery and `--help` do not open the database.

**Automatic migration from 0.6.1:** the first ordinary mailbox command backs up and upgrades schema 1 or 2, including databases selected through `HTALK_DB` or the default location. No extra command is needed. The published 0.6.0 package still uses the [explicit procedure documented with that release](https://github.com/jointsome0-lgtm/harness-talk/blob/v0.6.0/docs/reference.md#database-and-peers).

Before changing a legacy mailbox, htalk takes SQLite's write reservation and checks its version, required tables and columns, integrity and foreign keys. A schema-1 or schema-2 mailbox must have the legacy `peers` and `messages` columns. `url` and `wait_returned_at` may be absent; migration adds them. Existing `waits` and `retired_peers` tables must have their required columns, and absent tables are created. Column-name checks follow SQLite's case-insensitive identifier rules and allow additional columns. Missing tables or required columns return `invalid_database_schema` before a new backup is made. Broken references and integrity failures also stop before backup, so repeated opens rejected by these checks do not accumulate copies. It then creates a consistent backup, verifies the copy's schema and integrity and syncs it to disk. Backups are retained beside the mailbox as `PATH.backups/schema-OLD-before-3-UUID.sqlite3`; new backup directories have mode 0700 and files have mode 0600. Migration runs in the same transaction, rechecks foreign keys, and preserves existing peers as native, messages and receipts, replies, wait registrations and retirements. The original command continues after the migration commits, with its usual JSON result.

Concurrent first opens coordinate through SQLite: one client performs the migration. Others wait using the existing lock timeout; a large mailbox or slow disk can make them return `database is locked` before migration finishes. Retry after it completes. A database already on schema 3 needs no migration backup or writer lock on open. Unknown schema versions are refused without migration. `htalk migrate` remains an optional way to prepare a mailbox. It uses the same backup and migration path, selecting the database through `--db`, `HTALK_DB` or the default location like other commands.

If a backup cannot be completed, `database_backup_failed` reports its directory and the mailbox is not migrated. Check storage space, directory access and SQLite integrity before retrying. A failure after backup rolls back the migration and retains the completed copy. These preliminary checks do not guarantee that every later migration statement or commit will succeed; retrying a later failure can create another completed backup. Existing backups are never deleted by schema validation. For `invalid_database_schema`, preserve the mailbox and existing backups, inspect the layout on a copy, and recover a supported schema with matching clients before retrying. Do not assign a schema version manually. A failed migration does not run the requested mailbox operation. There is no automatic restore: replacing a mailbox with an older backup could discard messages saved since that backup. If manual recovery is necessary, stop access, preserve the current database with SQLite's backup API, and use a verified backup with matching clients.

Update every htalk installation sharing the mailbox. Clients 0.5.1 and earlier reject schema 3 when starting a command. An older command already in flight may complete against the migrated mailbox; that does not make the older binary compatible with pull peers or future schemas. A long migration can also delay an in-flight command's receipt write. If its notification outcome is uncertain, recover by the saved message ID rather than sending a new request.

Only `peer add` creates a missing database file and its directory. Other commands, including `migrate`, report `database_not_found` with `resolved_path`. An existing empty file is initialized on first open. Access and corruption errors are reported separately.

Contended `send` and `reply` calls coordinate through an empty directory named `DATABASE-htalk-turn` beside the database. Creation requests mode 0700; existing directory permissions are not changed. Leave it in place while clients are running. A call can wait up to five seconds for admission before falling back to SQLite's ordinary lock waiting. If the directory cannot be created or opened, including when another OS user cannot open it, the call falls back to SQLite's ordinary lock waiting without admission.

After acquiring admission, a call may also probe the writer's release with an empty immediate SQLite transaction. The probe changes no rows and, on success, explicitly rolls back before the application transaction begins. Its busy callback requests at most 100 one-millisecond sleeps, a maximum of 100 nominal milliseconds. A successful probe that waited can add another short grace pause. Probe errors skip that extra pause; the command continues subject to its ordinary error and interruption handling.

Admission, the optional probe and SQLite's ordinary lock waiting have separate budgets. Grace pauses, scheduling and other work add elapsed time, so these budgets do not form a total wall-clock command deadline or guarantee service order. SQLite still controls database access, and admission does not change transaction durability. The admission lock is released before notifying a client or waiting for an answer.

`peer add` records an immutable name, harness label and delivery mode. Native delivery is the default and requires a session address. It does not notify the session. `peer list` shows registered addresses; `peer discover` finds addresses outside the registry. Session IDs are UUIDs for Codex/Claude and opaque `ses...` IDs for OpenCode. Workspace comparisons resolve paths. Endpoint options must agree with the selected client.

`peer add NAME --harness LABEL --delivery pull` registers an agent that polls `inbox`. Labels use the same 1–64 lowercase letter/digit/underscore/hyphen syntax as names. Multiple pull peers may share a label. Session, workspace, socket and URL must be absent. Pull peer JSON includes `delivery: pull` with null address fields; native peer JSON retains its existing seven keys, with omitted delivery meaning native. `peer check` returns `status: pull_only`, not a presence claim. Pull registration never discovers or launches a client. Unknown harness labels do not break storage reads; an unavailable native adapter reports `adapter_unavailable` on check or notification, while its saved messages remain accessible.

`peer retire NAME` marks a peer whose session should get no new requests, for example after the session was replaced. New requests to or from that peer fail with `peer_retired`, and nothing is saved. Replies to saved requests still work. A notice to a retired recipient is skipped as `recipient_retired`, with exit 0. `inbox`, `show`, `wait` and `ack` are unaffected, and a recognized Claude Code session still selects its retired peer. `peer list` hides retired peers and counts them in `retired_hidden`; `peer list --all`, `peer check` and `peer add` show `retired_at`. Retiring again keeps the first time. `peer add` does not clear the mark, and the name and session stay bound; a registration conflict with a retired peer points `recovery.peers` to `peer list --all`. `peer restore NAME` allows new requests again but does not send skipped notices.

`--as NAME` overrides `HTALK_PEER`. Without either, a message command run by a recognized Claude Code session acts as the peer registered for that session. Results report the choice as `actor_source`: `option`, `HTALK_PEER` or `native_session`. Names are routing assertions under one trusted OS account. Anyone with database access can read or change it. For a Codex sender, a conflicting `CODEX_THREAD_ID` is rejected when available. For a Claude sender, a different recognized Claude Code session is rejected with `actor_conflicts_with_CLAUDE_CODE_SESSION_ID`. These checks apply only to native peers. A pull peer uses its explicit name even when its label is codex or claude. On a conflict, use the returned `recovery.peers` command to inspect addresses. Select the correct peer or register a separate name; existing peers cannot be reassigned.

htalk recognizes a Claude Code session only when all of these hold:

- `CLAUDE_CODE_SESSION_ID` is a UUID and `CLAUDE_PID` is a process ID.
- `~/.claude/sessions/CLAUDE_PID.json` records that session ID, that PID and the process start time shown in `/proc`.
- That process is an ancestor of htalk, with at most 64 processes in between.
- `CODEX_THREAD_ID` is unset, and no process in between has a command or executable name starting with `claude`, `codex` or `opencode`.

If any check fails, including when `/proc` or the metadata is unreadable, explicit names work as before. A command without one fails with `peer_required_use_as_or_HTALK_PEER`. Its `native_session` holds either the recognized `session_id` and `workspace` to register, or the `reason` recognition failed. Recognition reads htalk's own environment, that metadata file, and `/proc` entries for htalk's ancestors. It is not authentication: any process under the same account can reproduce this evidence.

Use `--as` or `HTALK_PEER` where recognition does not apply. A tmux server started by a Claude Code command detaches from Claude, so its panes are not recognized. A Claude Code session started from a Codex command inherits `CODEX_THREAD_ID` and is not recognized either. A client nested in a Claude Code command that sets neither Claude variable and runs under another process name, for example through `node`, inherits the outer session's peer. Recognition never selects a peer registered to another session ID, including after `/clear` or `--resume` changes the ID.

## Notifications and recovery

For a native recipient, after saving a message the store durably claims its one notification attempt. A pull recipient records `not_submitted` and `notification_detail: pull_only`, with null notification timestamps and no attempt. Acknowledgment cleanup is `skipped/pull_only`. The adapter then checks the exact recipient before client submission. An interruption during that check can leave `submission_unknown`; it does not permit another attempt. A notification contains a `show` command for the exact message ID, without the message body. An answer with `ack_at` set, or a request with a saved `reply`, needs no duplicate processing. A request (`in_reply_to` is null) without a reply stays open after acknowledgment and may still need an answer. The notification preserves the recipient's configured client/model settings and grants no additional permissions.

Run `peer check` in the scope that will send. `recipient_unavailable` can mean restricted discovery rather than an offline client. Use the client's normal permission approval for a needed host check; do not replay an already-saved notification. An unavailable client can later retrieve the message from `inbox`.

`notification_detail` and a cleanup `detail` carry fixed htalk codes, such as `recipient_unavailable` or `codex_rpc_rejected:-32600`. A failure below the client is recorded by a fixed code too, such as `file_not_found`, `connection_refused`, `timed_out` or `invalid_client_data`. [Error codes](#error-codes) has a row for each. No message text is saved.

| `submission` | Evidence |
| --- | --- |
| `not_submitted` | Disabled, never attempted, or rejected before transport. |
| `submission_unknown` | Claimed for one attempt, with an uncertain outcome. |
| `submitted` | Claude socket bytes written, Codex queue accepted, or OpenCode `prompt_async` accepted. |

None proves model receipt. `ack_at` records the recipient's explicit acknowledgment; it does not prove understanding or use of the message. A saved answer is a separate message with `in_reply_to` pointing to its request; the request becomes `reply_received` regardless of notification outcome. Neither an acknowledgment nor an answer proves task completion: the requester must check the result against the task's requirements. Acknowledged questions remain in the inbox until answered. Acknowledged answers leave the inbox. A short decline also counts as an answer.

`ack` saves the acknowledgment first, then tries to remove that message's pending Codex notification by its confirmed queue ID. Reading with `show`, `inbox` or `wait` does not remove it. If acknowledgment arrives during submission, the sender also tries removal when the queue receipt is saved.

A message acknowledged before the notification claim is not submitted. Every client route rechecks the saved state after its preflight, immediately before the one write: the Claude socket frame, the OpenCode `prompt_async` POST, the native `codex queue` command and the app-server `thread/queue/add` call. An acknowledgment received during preflight prevents transmission and records `not_submitted` with `acknowledged_before_notification`. The message and read mark remain saved; the one-attempt rule still applies.

`wait` and `send --wait` try to save `wait_returned_at` on an answer just before returning it, without waiting for a SQLite writer lock. This best-effort receipt suppresses redundant notices; a ready answer is still returned if the receipt cannot be saved, so a redundant notice may follow. The answer stays in the inbox, `ack_at` is untouched, and the receipt proves neither acknowledgment nor task completion. A claimed notification that sees this record at its final check is skipped with `not_submitted` and `returned_by_recipient_wait`, exit 0. The record can precede the notification attempt or appear during client preflight. `show`, `inbox` and `sent` write nothing.

This local record does not prove that command output reached the caller or that a model read it. A failure after the record is saved can suppress the notice even if the output is lost. Recover the saved answer through `show`, `wait` or `inbox` after interruption, then acknowledge it after reading.

A positive wait registers itself in an additive `waits` table only if its initial read finds no answer. `wait --seconds 0` and a wait that finds an answer immediately register nothing; both still try to save the return receipt without waiting for a writer lock. A reply saved during a registered wait gives the poll up to one second to record the answer before notifying. When the wait ends, times out or is interrupted, it tries to remove its registration. This cleanup is best effort: a busy database can leave the row behind. A registration never suppresses a notice by itself; a leftover row or a waiter killed before recording the answer can only delay notification by up to one second.

A race after the final check, including a notice already accepted by the client, remains possible. This check cannot recall such a notice or guarantee that it never causes another model turn.

The command's `notification_cleanup` describes this removal attempt. `removed` confirms deletion; `absent` means the queue ID was already gone. Neither can recall a notice already consumed by the client. `pending` means submission has started without a saved completion receipt. It provides `recovery.retry_notification_cleanup`, as do `unavailable` and `unknown`. The acknowledgment stays saved; after submission finishes, repeating `ack` safely retries removal without notifying again. An interrupted `ack` exits with code 130 and also provides this recovery command. `skipped` means there is no usable confirmed queue receipt, and `unsupported` means the client has no removal adapter. Cleanup does not change the historical `submission` receipt. Cleanup needs access to native Codex state or its registered socket. A writable htalk database does not imply that access: a sandboxed `ack` may save the read mark but return `unavailable`; use the client's normal approval flow for native access before retrying the listed command. Claude and OpenCode notices cannot currently be withdrawn.

If the sending process stops before saving its completion receipt, `pending` can remain indefinitely. Repeating `ack` preserves the read mark but cannot reconstruct the missing queue receipt or remove a notice without it. A child client that finishes after the sender stops does not update the htalk database. The saved message remains available through `show` and `inbox`; this uncertainty never permits another notification attempt.

`show` retrieves one message and its correlated answer. `inbox` finds incoming work, oldest first; `sent` recovers outgoing IDs, newest first. Both return at most `--limit` messages (default 20, up to 500) with `total`, the count of all matching messages, and `omitted`, the count beyond this page. When `omitted` is above 0, `recovery.next_page` continues with the same options from a `seq` cursor: `--after-seq` for `inbox`, `--before-seq` for `sent`. `sent` summarizes each text, including a correlated answer's, as `body_bytes` (UTF-8 size) and `body_preview`, the first nonblank line up to 120 characters; `show` and `sent --bodies` return full texts. Ordinary local CLI results carry executable `recovery` commands with the database, peer name and full IDs. Read an answer before executing `ack_after_reading`. [Catalogue reads](catalog.md#find-and-choose) retain IDs and saved state but omit these per-message action commands.

`wait REQUEST_UUID --seconds N` polls only the database for 0 to 45 seconds and tries to save the suppression receipt before returning an answer. A later notification check skips an answer carrying that record. Resume the wait after timeout or interruption. It never resends or acknowledges. `--no-notify` saves a message for polling only. `--message-file PATH` supplies a multiline body, including paths to artifacts the recipient should inspect.

For repeatable automation, generate and retain a UUID before `send --id UUID`. Identical retries return the saved request; differing contents are rejected. The same applies to an identical `reply` retry. There is no notification replay command. After interrupted output, inspect `recovery`, `message_id` and `persistence`; `unknown` persistence requires checking the database before deciding what happened.

Deduplication depends on the saved message in the selected database. A different mailbox or a restored backup that lacks that message cannot recognize the earlier request, so the same ID can create a new message. Reconcile the earlier exchange before retrying after a database replacement or restore.

## Notice stream

`htalk --as NAME watch` emits JSON lines for a harness extension. It first emits
`{"event":"ready","peer":"NAME"}`, then `message` events with `id`, `seq` and
`notification`. The notification contains a copyable `show` command and the same
peer-input boundary used by native adapters, without the peer's body.

It reads all open inbox pages and polls locally once per second. Each ID appears
once in that process; restarting replays still-open work. It neither acknowledges
messages nor changes notification receipts. Transient SQLite busy errors retry
the read. Other errors produce the usual error JSON and stop; SIGINT exits 130.
Closing the receiving pipe also stops the watcher, including while idle.

Use one receiver per peer. An emitted event proves only that a notice was written
to the pipe. The receiving harness still has to queue it and the agent has to read,
answer and acknowledge through the existing CLI. See the [session receiver
setup](../integrations/README.md) for session lifecycle and recovery behavior.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Local operation succeeded, including pull delivery, `--no-notify`, an identical retry, acknowledgment before notification completed, an answer already returned by the requester's wait, a notice skipped for a retired recipient, or `send --wait` that returned a saved answer. |
| 2 | Invalid input, a missing database, all discovery sources unavailable, or this invocation attempted notification without confirmed submission, acknowledgment or a saved answer. The message may already be saved. |
| 130 | Interrupted; read the recovery guidance. |

Retrieval, acknowledgment and identical retries can exit 0 even if the original notification failed. A `send --wait` that returns an answer exits 0 and keeps its request's unconfirmed `submission`. Acknowledgment also exits 0 if queue cleanup fails; inspect `notification_cleanup` separately. Mailbox storage is local SQLite on its host. Remote clients can use SSH connectors and the catalogue transport. htalk installs no daemon and has no Boardmail dependency.

## Error codes

A failed command answers `"state":"error"` with one of these codes as `error`. The same codes are the values of `notification_detail`, of a cleanup `detail`, and of the `error` or `detail` of a discovery source. `{}` stands for a part filled in at run time. `tests/test_agent_view.py` compares this table with the codes in the source, so a code without a row fails the tests.

### Arguments

| Code | What it means | What to do |
| --- | --- | --- |
| `claude_socket_is_discovered_from_live_identity` | `--socket` was given for a Claude Code peer. | Leave out `--socket`. htalk finds the socket from the live session. |
| `codex_socket_requires_codex_discovery` | `--codex-socket` was given while `peer discover` searched another harness. | Use `--harness codex` or leave out `--codex-socket`. |
| `invalid_arguments` | `peer add` for native delivery had no `--session` or no `--workspace`. | Give both `--session` and `--workspace`. |
| `invalid_harness_id` | The harness label is not 1 to 64 lowercase letters, digits, `_` or `-`. | Give a label in that form. |
| `invalid_opencode_session_id` | The OpenCode session id does not start with `ses`, or has characters outside letters, digits, `_.-`. | Copy the session id from `peer discover --harness opencode`. |
| `invalid_opencode_url` | The OpenCode server URL is not a plain `http` or `https` URL with a host. | Give a URL such as `http://127.0.0.1:4096`, without user or query. |
| `invalid_peer_name` | A peer name is not 1 to 64 lowercase letters, digits, `_` or `-`. | Give a name in that form, starting with a letter or digit. |
| `invalid_uuid` | A value is not a UUID: a message id, a session id, a profile id. | Give the full UUID. |
| `limit_must_be_between_1_and_500` | `--limit` is not a whole number from 1 to 500. | Give a limit from 1 to 500. |
| `message_must_be_1_to_32000_bytes` | The message text is empty, only whitespace, or longer than 32000 bytes. | Send a text of 1 to 32000 bytes. |
| `opencode_url_must_be_loopback` | The OpenCode server URL names a host other than `localhost` or a loopback address. | Use the URL of a server on this machine. |
| `opencode_url_requires_opencode_discovery` | `--opencode-url` was given while `peer discover` searched another harness. | Use `--harness opencode` or leave out `--opencode-url`. |
| `opencode_uses_a_server_url_not_a_socket` | `--socket` was given for an OpenCode peer. | Use `--url` with the server URL and leave out `--socket`. |
| `pull_peer_has_no_native_address` | `--session`, `--workspace`, `--socket` or `--url` was given with `--delivery pull`. | Leave those options out for a pull peer. |
| `seq_cursor_must_be_a_positive_integer` | The page cursor is not a whole number of 1 or more. | Use the cursor a previous page returned. |
| `unsupported_delivery` | The delivery mode is not `native` or `pull`. | Use `--delivery native` or `--delivery pull`. |
| `unsupported_harness` | The harness has no native adapter. The adapters are `codex`, `claude` and `opencode`. | Name one of the three, or register with `--delivery pull`. |
| `url_is_only_for_opencode` | `--url` was given for a Claude Code or Codex peer. | Leave out `--url`. |
| `wait_seconds_must_be_between_0_and_45` | The wait time is not a number from 0 to 45. | Give a number of seconds from 0 to 45. |
| `workspace_must_be_a_directory` | The `--workspace` path is not a directory. | Give the directory the session runs in. |

### Who is calling

| Code | What it means | What to do |
| --- | --- | --- |
| `actor_conflicts_with_CLAUDE_CODE_SESSION_ID` | The chosen peer belongs to another Claude Code session than the one running the command. | Check `--as` and `HTALK_PEER`, or run it in the registered session. |
| `actor_conflicts_with_CODEX_THREAD_ID` | The chosen Codex peer belongs to another session than `CODEX_THREAD_ID` names. | Use the peer of this session, or run it in the registered session. |
| `peer_required_use_as_or_HTALK_PEER` | No peer was named, and no registered Claude Code session was recognised. | Give `--as NAME` or set `HTALK_PEER`. |
| `unknown_peer` | No peer with that name is registered in this mailbox. | Check the name with `htalk peer list`. |

### Registration

| Code | What it means | What to do |
| --- | --- | --- |
| `peer_already_has_a_different_address` | The peer name is already registered with another harness, session, workspace, socket, URL or delivery. | Keep the registered address, or choose another name for the new session. |
| `peer_retired` | A new request names a retired peer, or a retired peer was given to `catalog publish`. | Choose an active peer. Replies to saved requests still work. |
| `session_already_has_a_peer_name` | The session is already registered under another peer name. | Use the name it has. `htalk peer list --all` shows it. |

### Messages

| Code | What it means | What to do |
| --- | --- | --- |
| `message_id_conflict` | The given message id is already used by a different message. | Do not resend. Find your own ids with `htalk sent`. |
| `message_not_addressed_to_peer` | The message exists, and the acting peer is neither its sender nor its recipient. | Use the peer that sent or received it. |
| `only_recipient_can_ack` | No message with that id is addressed to the acting peer. | Acknowledge as the recipient. Check the id with `htalk inbox`. |
| `reply_address_mismatch` | The reply is not from the recipient of the request to its sender. | Reply as the peer the request was sent to. |
| `reply_conflict_existing_answer_preserved` | The request already has a reply with a different text. The saved reply was kept. | Do not resend. Read the saved reply with `htalk show ID`. |
| `reply_requires_a_request` | The message to reply to is itself a reply. | Send a new request with `htalk send`. |
| `sender_and_recipient_must_differ` | A peer tried to send a message to itself. | Name another peer as the recipient. |
| `unknown_message` | No message with that id is saved in this mailbox. | Check the id with `htalk inbox` or `htalk sent`. |
| `unknown_request` | The request to reply to is not saved in this mailbox. | Check the request id with `htalk inbox`. |
| `wait_requires_own_request` | `wait` was given a reply, or a request another peer sent. | Wait on a request the acting peer sent. |

### The mailbox file

| Code | What it means | What to do |
| --- | --- | --- |
| `database_backup_failed` | The backup before migration could not be saved and verified. The mailbox was not migrated. | Check free space and write access to the backup directory, then retry. |
| `database_corrupt` | A catalogue command found the mailbox file damaged or not an SQLite database. | Check the mailbox path. Restore the file from a backup. |
| `database_foreign_key_violation` | The mailbox holds rows that point to missing rows. Migration was rolled back. | Repair a copy or restore a valid backup, then retry. |
| `database_integrity_check_failed` | SQLite's integrity check of the mailbox failed. The mailbox was not migrated. | Keep the file and its backups. Inspect them before a retry. |
| `database_not_found` | No mailbox file exists at the given path. | Check `--db` and `HTALK_DB`. Only `peer add` creates a mailbox. |
| `database_unavailable` | A catalogue command could not open or read the mailbox file. | Check the path and its permissions, then retry. |
| `invalid_database_schema` | A table or column that the mailbox's schema version needs is missing. | Check that this is an htalk mailbox. Restore a valid backup. |
| `invalid_notification_result` | A saved message has a notification result other than the three htalk writes. | Restore the mailbox from a valid backup. |
| `unsupported_database_version` | The mailbox has a schema version this htalk does not know. | Use the htalk version that wrote it. Do not edit the version. |
| `unsupported_unversioned_database` | The file has tables and no htalk schema version. It was not changed. | Check that this is the intended mailbox. Restore a valid backup. |

### Notification, any client

| Code | What it means | What to do |
| --- | --- | --- |
| `adapter_unavailable` | The recipient is a native peer whose harness has no adapter in this htalk. | Nothing was sent. The recipient reads the message with `htalk inbox`. |
| `client_has_no_notification_removal` | The recipient's client cannot take back a notice. The acknowledgment was saved. | Nothing; it tells that the notice stays in the client. |
| `invalid_native_address` | The recipient is a native peer saved without a session id or workspace. | Nothing was sent. Check the peer with `htalk peer list`. |
| `message_not_acknowledged_by_recipient` | Cleanup was skipped: the recipient has not acknowledged the message. | Nothing; cleanup runs when the recipient acknowledges. |
| `notification_message_not_found` | When the notice was about to go out, the message was not found for that recipient. | Check the message with `htalk show ID`. |
| `notification_state_unavailable` | The mailbox could not be read or written while a notice or cleanup was recorded. | Check the message with `htalk show ID`. Do not repeat the notice. |
| `pull_only` | The recipient is a pull peer. No notice is sent and none is cleaned up. | Nothing; the recipient reads the message with `htalk inbox`. |
| `recipient_identity_changed` | The client's session id or workspace no longer matches the registered peer. | Nothing was sent. Check the peer with `htalk peer check NAME`. |
| `recipient_socket_unavailable` | The client's socket path is not a socket owned by the current user. | Check that the client runs as the same user, then retry. |

### Claude Code

| Code | What it means | What to do |
| --- | --- | --- |
| `claude_socket_bytes_written` | The notice was written to the session's socket. That does not prove it was read. | Nothing; a reply or an acknowledgment shows it was read. |
| `invalid_claude_agents_response` | `claude agents --json` did not print a list of session records. | Check the installed Claude Code version. |
| `recipient_unavailable` | Not exactly one live Claude Code session has the peer's session id and workspace. | Start or resume that session in its workspace, then retry. |

### Codex

| Code | What it means | What to do |
| --- | --- | --- |
| `codex_cli_invalid_queue_id` | The queue id in the `codex queue` receipt or the saved detail is not a UUID. | Check the Codex version. Do not repeat the notice. |
| `codex_cli_queued:{}` | `codex queue` accepted the notice. The part after the colon is its queue id. | Nothing; it tells that the notice is queued for the session. |
| `codex_cli_unconfirmed_receipt` | `codex queue` failed or printed an unfamiliar receipt. The notice may be queued. | Do not repeat the notice. Check the session in Codex. |
| `codex_discovery_limit` | Discovery stopped at 200 loaded sessions or 20 pages from one Codex server. | Nothing can be done. The sessions found so far are listed. |
| `codex_discovery_time_limit` | Discovery on one Codex server took longer than 15 seconds. | Retry later. The sessions found so far are listed. |
| `codex_queue_delete_receipt_invalid` | The Codex server's answer to removing a queued notice did not say whether it was deleted. | Check the session in Codex. The acknowledgment was saved. |
| `codex_queue_receipt_mismatch` | The Codex server's queue receipt names another message than the one sent. | Do not repeat the notice. Check the session in Codex. |
| `codex_queued:{}` | The Codex server queued the notice. The part after the colon is its queue id. | Nothing; it tells that the notice is queued for the session. |
| `codex_rpc_closed` | The connection to the Codex server or to `codex app-server` ended before an answer. | Check that Codex is running, then check the peer. |
| `codex_rpc_frame_too_large` | `codex app-server` sent one answer line larger than htalk accepts. | Check the Codex version. |
| `codex_rpc_invalid_response` | A Codex answer was not UTF-8, not a JSON object, or had no result. | Check the Codex version. |
| `codex_rpc_rejected` | The Codex server refused the request and gave no numeric error code. | Check the peer with `htalk peer check NAME`. |
| `codex_rpc_rejected:{}` | The Codex server refused the request. The part after the colon is its JSON-RPC error code. | Check the peer with `htalk peer check NAME`. |
| `codex_rpc_timeout` | The Codex server did not answer one request within its time limit. | Retry later, or check that Codex is responsive. |
| `codex_server_socket_changed` | The Codex server socket was replaced after `htalk receive` selected it. | Restart `htalk receive` against the running server. |
| `codex_stdio_cleanup_ownership_unavailable` | htalk could not take charge of the `codex app-server` it started, and stopped it. | Retry. If it repeats, look for a leftover `codex app-server` process. |
| `codex_stdio_cleanup_unconfirmed` | htalk could not confirm that the `codex app-server` it started has exited. | Look for a leftover `codex app-server` process. |
| `codex_store_changed` | The Codex state database is no longer the file `htalk receive` started with. | Restart `htalk receive`. |
| `codex_store_name_unsupported` | The Codex state database that `htalk receive` uses is not named `state_5.sqlite`. | Use a Codex version that keeps its state in `state_5.sqlite`. |
| `codex_store_path_not_utf8` | The directory of the Codex state database has a name that is not UTF-8. | Move the Codex home to a path with a UTF-8 name. |
| `codex_websocket_failure` | The WebSocket connection to the Codex server socket failed or was closed by the server. | Check that the Codex server runs on that socket. |
| `invalid_codex_config` | The Codex configuration file is not UTF-8 or not valid TOML. | Fix the Codex configuration file. |
| `invalid_discovery_cursor` | A Codex server's list of loaded sessions gave a repeated or malformed page cursor. | Check the Codex version. The sessions found so far are listed. |
| `invalid_loaded_threads_response` | A Codex server's list of loaded sessions was not in the expected form. | Check the Codex version. |
| `invalid_lock_table` | The kernel's lock table, `/proc/locks`, had a line htalk could not read. | Nothing can be done. Discovery of Codex writers is incomplete. |
| `no_confirmed_queue_receipt` | Cleanup was skipped: the message has no saved queue id of a submitted notice. | Nothing; there is no queued notice htalk can remove. |
| `notification_submission_has_no_completion_receipt` | Cleanup is pending: the notice was started and its result is not yet saved. | Nothing; check the message again with `htalk show ID`. |
| `recipient_is_not_an_unarchived_codex_cli_session` | The Codex session is archived, or was not started by the Codex command line. | Register a session that is open in the Codex command line. |
| `recipient_not_in_codex_state` | The Codex state database has no session with the peer's session id. | Check the session id with `htalk peer discover --harness codex`. |
| `recipient_not_loaded` | The Codex server knows the session, and it is not loaded as idle or active. | Open the session in Codex, then retry. |

### OpenCode

| Code | What it means | What to do |
| --- | --- | --- |
| `invalid_opencode_credentials` | `OPENCODE_SERVER_PASSWORD` or `OPENCODE_SERVER_USERNAME` is not valid UTF-8. | Set both to the text the server expects. |
| `opencode_http_{}` | The OpenCode server answered with the HTTP status that ends the code. | Check the server. Do not repeat a notice saved as `submission_unknown`. |
| `opencode_invalid_response` | The OpenCode server's answer was not valid HTTP, or not the JSON expected. | Check that the URL is an OpenCode server, and its version. |
| `opencode_invalid_saved_metadata` | A row of OpenCode's saved sessions could not be used. The other rows are listed. | Check OpenCode's version. |
| `opencode_invalid_session_records` | A session the OpenCode server listed lacked the expected fields. The others are listed. | Check the server's version. |
| `opencode_prompt_async_accepted` | The OpenCode server accepted the notice. That does not prove it was read. | Nothing; a reply or an acknowledgment shows it was read. |
| `opencode_response_too_large` | The OpenCode server's answer was larger than 4 MiB. | Check that the URL is an OpenCode server. |
| `opencode_saved_metadata_missing` | OpenCode's saved sessions database is not at the path the source names. | Nothing; sessions of running servers are still listed. |
| `opencode_saved_session_limit_reached` | More than 50 saved OpenCode sessions exist. The 50 updated last are read. | Register an older session by its id with `htalk peer add`. |
| `opencode_saved_{}` | The saved OpenCode sessions database could not be read. The rest is the system code. | Look up the system code. Sessions from running servers are still listed. |
| `opencode_unauthorized` | The OpenCode server answered 401: the credentials are missing or wrong. | Set `OPENCODE_SERVER_PASSWORD`, and `OPENCODE_SERVER_USERNAME` if the server uses one. |
| `opencode_unreachable` | No connection to the OpenCode server was made, so nothing was sent. | Start the server or correct the URL, then check the peer. |
| `opencode_{}` | The request to the OpenCode server failed after it was sent. The rest names the failure. | Do not repeat the notice. Check the server and the session. |
| `recipient_not_in_opencode_server` | The OpenCode server answered 404 for the session or for the server itself. | Check the URL and session id with `htalk peer discover --harness opencode`. |
| `recipient_session_archived` | The recipient's OpenCode session is archived. | Restore the session in OpenCode, or register the peer's current session. |

### The system under a client

| Code | What it means | What to do |
| --- | --- | --- |
| `client_database_corrupt` | A client's database, Codex state or OpenCode saved sessions, is damaged or not SQLite. | Check the client. htalk does not repair its files. |
| `client_database_unavailable` | A client's database could not be opened or read. | Retry later, or check the client's files and their permissions. |
| `command_failed` | `claude agents --json` exited with a failure. | Run that command by hand to see why. |
| `command_timed_out` | A client's command did not finish within its time limit and was stopped. | Retry later, or run the client's command by hand. |
| `connection_closed` | The client closed the connection, or the pipe to it broke. | Check that the client is running, then check the peer. |
| `connection_refused` | Nothing listens on the client's socket or port. | Start the client, then check the peer. |
| `file_not_found` | A file, socket or program the command needs does not exist. | Check the path, or that the client is installed and running. |
| `interrupted` | The command was stopped by a signal before it finished. | Run it again. Check a message with `htalk show ID` first. |
| `invalid_client_data` | A client's answer or file did not have the fields its protocol names. | Check the client's version. |
| `invalid_json` | A client's answer or a file that should be JSON is not valid JSON. | Check the file, or the client's version. |
| `invalid_utf8` | A client's output, file or database text is not valid UTF-8. | Check the client's files. |
| `os_error` | The operating system failed in a way that has no code of its own. | Retry later, or check the client. |
| `permission_denied` | The current user may not read, write or run a file, socket or program. | Check the owner and mode of the path. |
| `timed_out` | The client did not connect or answer within the time limit. | Retry later, or check that the client is responsive. |

### Catalogue

| Code | What it means | What to do |
| --- | --- | --- |
| `catalog_ambiguous_profile` | `catalog connect` found more than one current profile with that id or name. | Name the profile by its id, or remove the duplicate. |
| `catalog_binding_fixed_omit_db_and_as` | `--db` or `--as` was given to a catalogue command other than `publish`. | Leave out `--db` and `--as`. The catalogue file fixes both. |
| `catalog_config_must_be_private_owned_file` | A catalogue, trust or binding file is not a regular private file, or is too large. | Make it a regular file only the current user can read and write. |
| `catalog_config_requires_directory` | The `--config` path has no parent directory. | Give a path to a file inside a directory. |
| `catalog_config_too_large` | The catalogue file, or a file read like it, is over the size limit. | Remove profiles, or check that the path names the right file. |
| `catalog_conversation_unavailable` | A remote caller named a message that is unknown or is with an unpublished peer. | Use a message exchanged with a published profile. |
| `catalog_directory_not_owned` | The catalogue file's directory is not owned by the current user, or others can write it. | Own the directory and remove write access for others. |
| `catalog_discovery_budget_exhausted` | Discovery ran out of time before it could ask this device. | Retry, or raise `--seconds` where the command has it. |
| `catalog_duplicate_profile` | The same profile id appears twice in a catalogue or binding file. | Remove the duplicate profile. |
| `catalog_endpoint_binding_mismatch` | `--db` or `--as` differs from the catalogue file, or a device differs from the trust file. | Use the mailbox and sender of the catalogue, or update the trust file. |
| `catalog_interface_not_found` | The `--interface` name is malformed or is not a network interface of this machine. | Give an interface that `ip link` lists. |
| `catalog_interface_unavailable` | The network interface is not up or cannot carry multicast. | Bring the interface up, or choose another. |
| `catalog_invalid_advertisement` | The mDNS record for this device could not be built. | Check the device id and SSH port in the catalogue file. |
| `catalog_invalid_binding` | The binding file has an unknown version, more than 64 profiles, or none. | Use the binding file as it was written. Do not edit it. |
| `catalog_invalid_config` | The catalogue file has an unknown version, port 0, over 64 profiles or a relative mailbox. | Publish again into a new catalogue file. |
| `catalog_invalid_lock` | The lock file beside the catalogue file is not a regular private file. | Remove the lock file and retry. |
| `catalog_invalid_profile` | A device's directory has a repeated profile id or an unknown delivery, state or status. | Check that both sides run the same htalk version. |
| `catalog_invalid_ssh_address` | An `ssh_address` in the trust file is not a usable IPv4 address. | Write the device's IPv4 address. |
| `catalog_invalid_ssh_file` | An identity or known-hosts file in the trust file is not an absolute, owned, protected file. | Give absolute paths to files only the current user can write. |
| `catalog_invalid_text` | A name, role, device name or profile selector is empty, too long or has control characters. | Shorten the text and remove control characters. |
| `catalog_invalid_trust` | The trust file has an unknown version, over 16 devices, or a repeated or malformed device. | Correct the device's id, SSH user, SSH port or Tailscale id. |
| `catalog_ipv4_unavailable` | An advertised device gave no usable IPv4 address on the chosen interface. | Check that both devices have IPv4 on that network. |
| `catalog_mailbox_not_owned_file` | The mailbox path is not a regular file owned by the current user. | Give the path of the mailbox file itself, owned by this user. |
| `catalog_mailbox_replaced` | The mailbox file is not the same file as when the catalogue was first published. | Publish again into a new catalogue file. |
| `catalog_mcp_failed` | The MCP server of a catalogue connection stopped with an error. | Run `catalog connect` again. Check the device with `catalog discover`. |
| `catalog_mdns_announcement_unconfirmed` | The mDNS announcement was not confirmed within 5 seconds. Nothing is advertised. | Check the interface and retry. |
| `catalog_mdns_shutdown_unconfirmed` | The mDNS service did not confirm that it stopped within 2 seconds. | Nothing can be done. The process ends anyway. |
| `catalog_mdns_unavailable` | The mDNS service could not start, browse or register on the interface, or reported an error. | Check the interface and retry. |
| `catalog_mdns_unregister_unconfirmed` | The mDNS record was not confirmed as withdrawn when advertising stopped. | Nothing can be done. Other devices drop the record when it expires. |
| `catalog_pan_binding_changed` | The Bluetooth network connection changed between discovery and use. | Run the command again. |
| `catalog_pan_unavailable` | Not exactly one active Bluetooth network connection was found on the interface. | Connect the Bluetooth network, then check it with `nmcli`. |
| `catalog_profile_not_published` | A remote caller named a peer that is not a published profile. | Publish the peer with `catalog publish`, or name a published one. |
| `catalog_profile_unavailable` | `catalog connect` found no current profile with that id or name. | List the profiles with `catalog discover`. |
| `catalog_publish_requires_db_and_as` | The first `catalog publish` into a new catalogue file had no `--db` or no `--as`. | Give both `--db` and `--as` before `catalog publish`. |
| `catalog_remote_command_refused` | `catalog serve` was started with an SSH command other than `catalog` or `mcp`. | Let the SSH key run only those two commands. |
| `catalog_requires_canonical_uuid` | An id is a UUID that is not in lowercase form with hyphens. | Write the id in lowercase with hyphens. |
| `catalog_requires_schema3` | The mailbox is not at the current schema version. | Run `htalk --db FILE migrate`, then retry. |
| `catalog_route_interface_changed` | The route to the device no longer leaves through the chosen interface. | Check the network, then run the command again. |
| `catalog_sender_binding_changed` | The sender peer of the catalogue was retired, or its registration changed. | Restore the peer, or publish again into a new catalogue file. |
| `catalog_ssh_not_bound` | A trusted device has no `ssh_address` in the trust file. | Add the device's `ssh_address`, or use another `--via`. |
| `catalog_ssh_omit_interface` | `--interface` was given with `--via ssh`. | Leave out `--interface`. |
| `catalog_ssh_port_mismatch` | A device found on the local network advertises another SSH port than its configured one. | Correct `ssh_port` for the device in the catalogue configuration, or check the device. |
| `catalog_tailscale_binding_changed` | The Tailscale identity or the device's address changed between discovery and use. | Run the command again. |
| `catalog_tailscale_invalid_binary` | The Tailscale program path is not an absolute executable file owned by root or this user. | Give the path of the installed `tailscale` program. |
| `catalog_tailscale_invalid_socket` | The Tailscale socket path is not a socket in a directory closed to other users. | Give the path of the `tailscaled` socket. |
| `catalog_tailscale_invalid_status` | The output of `tailscale status --json` could not be read. | Check the Tailscale version. |
| `catalog_tailscale_not_bound` | A trusted device has no `tailscale_peer_id` in the trust file. | Add the device's `tailscale_peer_id`, or use another `--via`. |
| `catalog_tailscale_omit_interface` | `--interface` was given with `--via tailscale`. | Leave out `--interface`. |
| `catalog_tailscale_peer_unavailable` | Tailscale does not list the device, or lists it without an address. | Check that the device is online in Tailscale. |
| `catalog_tailscale_unavailable` | `tailscale status` failed, or Tailscale is not running and online. | Start Tailscale and log in. |
| `catalog_tailscale_unsupported_version` | The Tailscale version is not 1.102. | Install Tailscale 1.102. |
| `catalog_too_many_profiles` | The catalogue file already holds 64 profiles. | Unpublish a profile first. |
| `catalog_unknown_profile` | `catalog unpublish` named a profile id that the catalogue file does not hold. | List the ids with `catalog export`. |
| `catalog_unreachable` | SSH to the device, or a network tool, failed, timed out or answered too much. | Check that the device is on and reachable over SSH. |
| `catalog_writer_active` | Another catalogue command holds the lock of this catalogue file. | Wait for it to finish, then retry. |

## Source installation and tests

Source installs compile Rust and need Rust 1.88+ plus a C compiler/linker. Binary wheels do not need a compiler. See [development](development.md) for the build, isolated fixture tests and installed-wheel checks.

```sh
python3 -m venv .venv
.venv/bin/pip install .
.venv/bin/htalk --help
cargo test --locked
cargo build --locked
python3 -B -m unittest discover -s tests -p test_cli_compat.py -v
```

SQLite is bundled in the executable. Ordinary Codex notifications use `codex queue`; the explicit app-server socket uses a native WebSocket connection. Claude uses its Unix socket, and OpenCode uses its local HTTP/HTTPS server.

## Remote mailbox access

`htalk mcp --connect -- COMMAND ARGS...` keeps a local MCP server alive while
connecting separately for each call to a fixed remote endpoint.
`htalk receive --peer NAME --session UUID --workspace PATH --state DIRECTORY -- COMMAND ARGS...`
forwards its remote watch stream into one existing Codex CLI session, with
durable notification receipts. Both commands are available in 0.9.0. They do
not select a local database with `--db` or `--as`. See [setup and recovery](../integrations/remote.md).

In 0.9.3, add `--mcp-command /absolute/mcp-connector` to `receive` to check
current mail before submitting a notice. The executable takes no arguments
and must reach the same fixed MCP mailbox and peer. It becomes part of the
saved binding; an existing state can add its first checker without losing
receipts. Completed messages are skipped, but ACKed unanswered requests still
wake. Without this option the previous watch-only behavior remains.
