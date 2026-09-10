# htalk reference

Start with the [first exchange](../README.md). Commands print JSON; `--help` describes their arguments. [Client adapters](adapters.md) document notification and discovery compatibility.

## Database and peers

Choose one writable directory shared by the participants. SQLite writers need directory access for the journal as well as file access. The path is selected in this order:

1. `--db PATH`
2. `HTALK_DB`
3. `$XDG_DATA_HOME/harness-talk/mail.sqlite3`
4. `~/.local/share/harness-talk/mail.sqlite3`

A checkout is never the default database location. Before an existing database is opened by 0.3, update every participant: the first storage command migrates it to schema 2, preserving peers, messages and acknowledgments. Older clients reject it. The `waits` table and the nullable `messages.wait_returned_at` column are added to schema 2 files without changing the version; a 0.3.0 client ignores them, so its waits and replies keep the earlier notification behavior. Discovery does not open or migrate the database.

`peer add` records an immutable name and session address. It does not notify the session. `peer list` shows registered addresses; `peer discover` finds addresses outside the registry. Session IDs are UUIDs for Codex/Claude and opaque `ses...` IDs for OpenCode. Workspace comparisons resolve paths. Endpoint options must agree with the selected client.

`--as NAME` overrides `HTALK_PEER`. Names are routing assertions under one trusted OS account. Anyone with database access can read or change it. For a Codex sender, a conflicting `CODEX_THREAD_ID` is rejected when available; Claude/OpenCode actors ignore that inherited variable. On a conflict, use the returned `recovery.peers` command to inspect addresses. Select the correct peer or register a separate name; existing peers cannot be reassigned.

## Notifications and recovery

After saving a message, the store durably claims its one notification attempt. The adapter then checks the exact recipient before client submission. An interruption during that check can leave `submission_unknown`; it does not permit another attempt. A notification contains a `show` command for the exact message ID, without the message body. An answer with `ack_at` set, or a request with a saved `reply`, needs no duplicate processing. A request (`in_reply_to` is null) without a reply stays open after acknowledgment and may still need an answer. The notification preserves the recipient's configured client/model settings and grants no additional permissions.

Run `peer check` in the scope that will send. `recipient_unavailable` can mean restricted discovery rather than an offline client. Use the client's normal permission approval for a needed host check; do not replay an already-saved notification. An unavailable client can later retrieve the message from `inbox`.

| `submission` | Evidence |
| --- | --- |
| `not_submitted` | Disabled, never attempted, or rejected before transport. |
| `submission_unknown` | Claimed for one attempt, with an uncertain outcome. |
| `submitted` | Claude socket bytes written, Codex queue accepted, or OpenCode `prompt_async` accepted. |

None proves model receipt. `ack_at` records explicit acknowledgment. A saved answer is a separate message with `in_reply_to` pointing to its request; the request becomes `reply_received` regardless of notification outcome. Acknowledged questions remain in the inbox until answered. Acknowledged answers leave the inbox. A short decline also counts as an answer.

`ack` saves the acknowledgment first, then tries to remove that message's pending Codex notification by its confirmed queue ID. Reading with `show`, `inbox` or `wait` does not remove it. If acknowledgment arrives during submission, the sender also tries removal when the queue receipt is saved.

A message acknowledged before the notification claim is not submitted. Every client route rechecks the saved state after its preflight, immediately before the one write: the Claude socket frame, the OpenCode `prompt_async` POST, the native `codex queue` command and the app-server `thread/queue/add` call. An acknowledgment received during preflight prevents transmission and records `not_submitted` with `acknowledged_before_notification`. The message and read mark remain saved; the one-attempt rule still applies.

`wait` and `send --wait` return an answer by first recording on that answer that the recipient's wait returned it (`wait_returned_at`). This is a receipt of observation, not an acknowledgment: the answer stays in the inbox, `ack_at` is untouched, and questions are not closed. A claimed notification for an answer carrying that receipt is skipped with `not_submitted` and `returned_by_recipient_wait`, exit 0, whether the receipt already exists when the reply is saved or appears during the client preflight before the final check. `show`, `inbox` and `sent` write nothing.

While a wait is running it also registers itself in an additive `waits` table and removes that row when it ends, times out or is interrupted. The registration is only a hint: a reply saved during it gives the poll up to one second to return the answer and write the receipt before notifying. It never suppresses a notice by itself, so a killed waiter costs at most one redundant notice, never a missing wake-up. `wait --seconds 0` registers nothing but still records an answer it returns.

A race after the final check, including a notice already accepted by the client, remains possible. This check cannot recall such a notice or guarantee that it never causes another model turn.

The command's `notification_cleanup` describes this removal attempt. `removed` confirms deletion; `absent` means the queue ID was already gone. Neither can recall a notice already consumed by the client. `pending` means submission has started without a saved completion receipt. It provides `recovery.retry_notification_cleanup`, as do `unavailable` and `unknown`. The acknowledgment stays saved; after submission finishes, repeating `ack` safely retries removal without notifying again. An interrupted `ack` exits with code 130 and also provides this recovery command. `skipped` means there is no usable confirmed queue receipt, and `unsupported` means the client has no removal adapter. Cleanup does not change the historical `submission` receipt. Cleanup needs access to native Codex state or its registered socket. A writable htalk database does not imply that access: a sandboxed `ack` may save the read mark but return `unavailable`; use the client's normal approval flow for native access before retrying the listed command. Claude and OpenCode notices cannot currently be withdrawn.

`show` retrieves one message and its correlated answer. `inbox` finds incoming work; `sent` recovers outgoing IDs. Results carry executable `recovery` commands with the database, peer name and full IDs. Read an answer before executing `ack_after_reading`.

`wait REQUEST_UUID --seconds N` polls only the database for 0 to 45 seconds and records an answer it returns so that answer is not also announced to your client. Resume it after timeout or interruption. It never resends or acknowledges. `--no-notify` saves a message for polling only. `--message-file PATH` supplies a multiline body, including paths to artifacts the recipient should inspect.

For repeatable automation, generate and retain a UUID before `send --id UUID`. Identical retries return the saved request; differing contents are rejected. The same applies to an identical `reply` retry. There is no notification replay command. After interrupted output, inspect `recovery`, `message_id` and `persistence`; `unknown` persistence requires checking the database before deciding what happened.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Local operation succeeded, including `--no-notify`, an identical retry, acknowledgment before notification completed, or an answer already returned by the requester's wait. |
| 2 | Invalid input, all discovery sources unavailable, or this invocation attempted notification without confirmed submission or acknowledgment. The message may already be saved. |
| 130 | Interrupted; read the recovery guidance. |

Retrieval, acknowledgment and identical retries can exit 0 even if the original notification failed. Acknowledgment also exits 0 if queue cleanup fails; inspect `notification_cleanup` separately. There is no daemon, remote-host transport or Boardmail dependency.

## Source installation and tests

From a checkout:

```sh
python3 -m venv .venv
.venv/bin/pip install .
.venv/bin/htalk --help
PYTHONPATH=src .venv/bin/python -m unittest discover -s tests -v
```

Storage, Claude notifications and OpenCode HTTP use the standard library. Ordinary Codex notifications use `codex queue`; the explicit app-server socket mode uses `websockets`.
