# htalk reference

Start with the [first exchange](../README.md). Commands print JSON; `--help` describes their arguments. [Client adapters](adapters.md) document notification and discovery compatibility.

## Database and peers

Choose one writable directory shared by the participants. SQLite writers need directory access for the journal as well as file access. The path is selected in this order:

1. `--db PATH`
2. `HTALK_DB`
3. `$XDG_DATA_HOME/harness-talk/mail.sqlite3`
4. `~/.local/share/harness-talk/mail.sqlite3`

A checkout is never the default database location. Before an existing database is opened by 0.3, update every participant: the first storage command migrates it to schema 2, preserving peers, messages and acknowledgments. Older clients reject it. Discovery does not open or migrate the database.

`peer add` records an immutable name and session address. It does not notify the session. `peer list` shows registered addresses; `peer discover` finds addresses outside the registry. Session IDs are UUIDs for Codex/Claude and opaque `ses...` IDs for OpenCode. Workspace comparisons resolve paths. Endpoint options must agree with the selected client.

`--as NAME` overrides `HTALK_PEER`. Names are routing assertions under one trusted OS account. Anyone with database access can read or change it. For a Codex sender, a conflicting `CODEX_THREAD_ID` is rejected when available; Claude/OpenCode actors ignore that inherited variable. On a conflict, use the returned `recovery.peers` command to inspect addresses. Select the correct peer or register a separate name; existing peers cannot be reassigned.

## Notifications and recovery

The database commits before client I/O. Identity preflight checks the exact recipient, then the store claims at most one notification attempt. A notification contains the inbox command and message ID, not the message body. It preserves the recipient's configured client/model settings and grants no additional permissions.

Run `peer check` in the scope that will send. `recipient_unavailable` can mean restricted discovery rather than an offline client. Use the client's normal permission approval for a needed host check; do not replay an already-saved notification. An unavailable client can later retrieve the message from `inbox`.

| `submission` | Evidence |
| --- | --- |
| `not_submitted` | Disabled, never attempted, or rejected before transport. |
| `submission_unknown` | Claimed for one attempt, with an uncertain outcome. |
| `submitted` | Claude socket bytes written, Codex queue accepted, or OpenCode `prompt_async` accepted. |

None proves model receipt. `ack_at` records explicit acknowledgment. A saved answer is a separate message with `in_reply_to` pointing to its request; the request becomes `reply_received` regardless of notification outcome. Acknowledged questions remain in the inbox until answered. Acknowledged answers leave the inbox. A short decline also counts as an answer.

`show` retrieves one message and its correlated answer. `inbox` finds incoming work; `sent` recovers outgoing IDs. Results carry executable `recovery` commands with the database, peer name and full IDs. Read an answer before executing `ack_after_reading`.

`wait REQUEST_UUID --seconds N` polls only the database for 0 to 45 seconds. Resume it after timeout or interruption. It never resends or acknowledges. `--no-notify` saves a message for polling only. `--message-file PATH` supplies a multiline body, including paths to artifacts the recipient should inspect.

For repeatable automation, generate and retain a UUID before `send --id UUID`. Identical retries return the saved request; differing contents are rejected. The same applies to an identical `reply` retry. There is no notification replay command. After interrupted output, inspect `recovery`, `message_id` and `persistence`; `unknown` persistence requires checking the database before deciding what happened.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Local operation succeeded, including an intentional `--no-notify` or an identical retry. |
| 2 | Invalid input, all discovery sources unavailable, or this invocation attempted notification without confirmed submission. The message may already be saved. |
| 130 | Interrupted; read the recovery guidance. |

Retrieval, acknowledgment and identical retries can exit 0 even if the original notification failed. Inspect the saved message's `submission` separately. There is no daemon, remote-host transport or Boardmail dependency.

## Source installation and tests

From a checkout:

```sh
python3 -m venv .venv
.venv/bin/pip install .
.venv/bin/htalk --help
PYTHONPATH=src .venv/bin/python -m unittest discover -s tests -v
```

Storage, Claude notifications and OpenCode HTTP use the standard library. Ordinary Codex notifications use `codex queue`; the explicit app-server socket mode uses `websockets`.
