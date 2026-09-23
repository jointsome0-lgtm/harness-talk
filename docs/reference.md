# htalk reference

Start with the [first exchange](../README.md). Commands print JSON; `--help` describes their arguments. [Client adapters](adapters.md) document notification and discovery compatibility.

## Database and peers

Choose one writable directory shared by the participants. SQLite writers need directory access for the journal as well as file access. The path is selected in this order:

1. `--db PATH`
2. `HTALK_DB`
3. `$XDG_DATA_HOME/harness-talk/mail.sqlite3`
4. `~/.local/share/harness-talk/mail.sqlite3`

A checkout is never the default database location. New databases use schema 3. Discovery and `--help` do not open the database.

**Unreleased automatic migration:** the first ordinary mailbox command backs up and upgrades schema 1 or 2, including databases selected through `HTALK_DB` or the default location. No extra command is needed. The published 0.6.0 package still uses the [explicit procedure documented with that release](https://github.com/jointsome0-lgtm/harness-talk/blob/v0.6.0/docs/reference.md#database-and-peers).

Before changing a legacy mailbox, htalk takes SQLite's write reservation, rechecks its version and creates a consistent backup. It verifies the backup's schema and integrity and syncs it to disk. Backups are retained beside the mailbox as `PATH.backups/schema-OLD-before-3-UUID.sqlite3`; new backup directories have mode 0700 and files have mode 0600. Migration then runs in one transaction, checks integrity and foreign keys, and preserves existing peers as native, messages and receipts, replies, wait registrations and retirements. The original command continues after the migration commits, with its usual JSON result.

Concurrent first opens coordinate through SQLite: one client performs the migration, and the others use the resulting schema. A database already on schema 3 needs no migration backup or writer lock on open. Unknown schema versions are refused without migration. `htalk --db PATH migrate` remains an optional way to prepare a mailbox; it uses the same backup and migration path and still requires an explicit `--db PATH`.

If a backup cannot be completed, `database_backup_failed` reports its directory and the mailbox is not migrated. Fix the reported storage or access problem and retry the original command. Failed schema or integrity checks roll back the migration; any completed backup is retained. A failed migration does not run the requested mailbox operation. There is no automatic restore: replacing a mailbox with an older backup could discard messages saved since that backup. If manual recovery is necessary, stop access, preserve the current database with SQLite's backup API, and use a verified backup with matching clients.

Update every htalk installation sharing the mailbox. Clients 0.5.1 and earlier reject schema 3 when starting a command. An older command already in flight may complete against the migrated mailbox; that does not make the older binary compatible with pull peers or future schemas.

Only `peer add` creates a missing database file and its directory. Other commands, including `migrate`, report `database_not_found` with `resolved_path`. An existing empty file is initialized on first open. Access and corruption errors are reported separately.

Contended `send` and `reply` calls coordinate through an empty directory named `DATABASE-htalk-turn` beside the database. Leave it in place while clients are running. A call can wait up to five seconds for admission before falling back to SQLite's ordinary lock waiting; SQLite still controls database access. This is an additional wait, not a total command deadline or a guarantee of service order. The admission lock is released before notifying a client or waiting for an answer.

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

`notification_detail` and a cleanup `detail` carry fixed htalk codes, such as `recipient_unavailable` or `codex_rpc_rejected:-32600`. Any other failure is recorded by its exception class name only, without its message text.

| `submission` | Evidence |
| --- | --- |
| `not_submitted` | Disabled, never attempted, or rejected before transport. |
| `submission_unknown` | Claimed for one attempt, with an uncertain outcome. |
| `submitted` | Claude socket bytes written, Codex queue accepted, or OpenCode `prompt_async` accepted. |

None proves model receipt. `ack_at` records explicit acknowledgment. A saved answer is a separate message with `in_reply_to` pointing to its request; the request becomes `reply_received` regardless of notification outcome. Acknowledged questions remain in the inbox until answered. Acknowledged answers leave the inbox. A short decline also counts as an answer.

`ack` saves the acknowledgment first, then tries to remove that message's pending Codex notification by its confirmed queue ID. Reading with `show`, `inbox` or `wait` does not remove it. If acknowledgment arrives during submission, the sender also tries removal when the queue receipt is saved.

A message acknowledged before the notification claim is not submitted. Every client route rechecks the saved state after its preflight, immediately before the one write: the Claude socket frame, the OpenCode `prompt_async` POST, the native `codex queue` command and the app-server `thread/queue/add` call. An acknowledgment received during preflight prevents transmission and records `not_submitted` with `acknowledged_before_notification`. The message and read mark remain saved; the one-attempt rule still applies.

`wait` and `send --wait` write `wait_returned_at` on an answer just before returning it. The answer stays in the inbox, `ack_at` is untouched, and questions are not closed. A claimed notification that sees this record at its final check is skipped with `not_submitted` and `returned_by_recipient_wait`, exit 0. The record can precede the notification attempt or appear during client preflight. `show`, `inbox` and `sent` write nothing.

This local record does not prove that command output reached the caller or that a model read it. A failure after the record is saved can suppress the notice even if the output is lost. Recover the saved answer through `show`, `wait` or `inbox` after interruption, then acknowledge it after reading.

While a wait is running it also registers itself in an additive `waits` table and removes that row when it ends, times out or is interrupted. A reply saved during it gives the poll up to one second to record the answer before notifying. The registration never suppresses a notice by itself: a waiter killed before recording the answer only delays notification by up to one second. `wait --seconds 0` registers nothing but still records an answer it returns.

A race after the final check, including a notice already accepted by the client, remains possible. This check cannot recall such a notice or guarantee that it never causes another model turn.

The command's `notification_cleanup` describes this removal attempt. `removed` confirms deletion; `absent` means the queue ID was already gone. Neither can recall a notice already consumed by the client. `pending` means submission has started without a saved completion receipt. It provides `recovery.retry_notification_cleanup`, as do `unavailable` and `unknown`. The acknowledgment stays saved; after submission finishes, repeating `ack` safely retries removal without notifying again. An interrupted `ack` exits with code 130 and also provides this recovery command. `skipped` means there is no usable confirmed queue receipt, and `unsupported` means the client has no removal adapter. Cleanup does not change the historical `submission` receipt. Cleanup needs access to native Codex state or its registered socket. A writable htalk database does not imply that access: a sandboxed `ack` may save the read mark but return `unavailable`; use the client's normal approval flow for native access before retrying the listed command. Claude and OpenCode notices cannot currently be withdrawn.

If the sending process stops before saving its completion receipt, `pending` can remain indefinitely. Repeating `ack` preserves the read mark but cannot reconstruct the missing queue receipt or remove a notice without it. A child client that finishes after the sender stops does not update the htalk database. The saved message remains available through `show` and `inbox`; this uncertainty never permits another notification attempt.

`show` retrieves one message and its correlated answer. `inbox` finds incoming work, oldest first; `sent` recovers outgoing IDs, newest first. Both return at most `--limit` messages (default 20, up to 500) with `total`, the count of all matching messages, and `omitted`, the count beyond this page. When `omitted` is above 0, `recovery.next_page` continues with the same options from a `seq` cursor: `--after-seq` for `inbox`, `--before-seq` for `sent`. `sent` summarizes each text, including a correlated answer's, as `body_bytes` (UTF-8 size) and `body_preview`, the first nonblank line up to 120 characters; `show` and `sent --bodies` return full texts. Results carry executable `recovery` commands with the database, peer name and full IDs. Read an answer before executing `ack_after_reading`.

`wait REQUEST_UUID --seconds N` polls only the database for 0 to 45 seconds and records an answer before returning it. A later notification check skips an answer carrying that record. Resume the wait after timeout or interruption. It never resends or acknowledges. `--no-notify` saves a message for polling only. `--message-file PATH` supplies a multiline body, including paths to artifacts the recipient should inspect.

For repeatable automation, generate and retain a UUID before `send --id UUID`. Identical retries return the saved request; differing contents are rejected. The same applies to an identical `reply` retry. There is no notification replay command. After interrupted output, inspect `recovery`, `message_id` and `persistence`; `unknown` persistence requires checking the database before deciding what happened.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Local operation succeeded, including pull delivery, `--no-notify`, an identical retry, acknowledgment before notification completed, an answer already returned by the requester's wait, a notice skipped for a retired recipient, or `send --wait` that returned a saved answer. |
| 2 | Invalid input, a missing database, all discovery sources unavailable, or this invocation attempted notification without confirmed submission, acknowledgment or a saved answer. The message may already be saved. |
| 130 | Interrupted; read the recovery guidance. |

Retrieval, acknowledgment and identical retries can exit 0 even if the original notification failed. A `send --wait` that returns an answer exits 0 and keeps its request's unconfirmed `submission`. Acknowledgment also exits 0 if queue cleanup fails; inspect `notification_cleanup` separately. There is no daemon, remote-host transport or Boardmail dependency.

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
