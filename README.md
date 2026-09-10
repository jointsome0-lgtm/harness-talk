# harness-talk

`htalk` discovers local Codex, Claude Code and OpenCode session addresses and saves messages between registered peers. Either side can ask, reply, wait now, or retrieve later. Messages live in one SQLite database; notifications point the recipient to its inbox.

Python 3.11 or newer. Storage, Claude notifications and OpenCode HTTP requests use the standard library. Ordinary Codex notifications use `codex queue`; app-server sockets use the `websockets` library. This version targets Linux and the client versions in [adapter notes](docs/adapters.md) and [OpenCode notes](docs/opencode.md).

## Install and share a database

Install the released command with `uv tool install 'harness-talk==0.3.0'`, or use a source checkout:

```sh
python3 -m venv .venv
.venv/bin/pip install .
export PATH="$PWD/.venv/bin:$PATH"
export HTALK_DB=/absolute/shared/writable/directory/mail.sqlite3
```

Install in a location both sessions can execute, and choose a database directory both sessions can write. Each SQLite writer also needs directory access for the journal. No global install, aliases, client configuration changes, or model processes are added. The optional alias `alias talk='htalk'` is a personal shell choice.

`--db PATH` overrides `HTALK_DB`. The default is `$XDG_DATA_HOME/harness-talk/mail.sqlite3`, or `~/.local/share/harness-talk/mail.sqlite3`. A project checkout is never the default storage location.

Upgrade every participant before opening an existing shared database with 0.3. The first storage command upgrades it to schema 2, preserving peers, messages and acknowledgments. Older htalk versions reject the upgraded database. Discovery does not open or upgrade the htalk database.

## Find session addresses

```sh
htalk peer discover
htalk peer discover --harness claude
htalk peer discover --harness codex --workspace /absolute/project
htalk peer discover --codex-socket /absolute/codex.sock
htalk peer discover --harness opencode --opencode-url http://127.0.0.1:4096 \
  --workspace /absolute/project
```

`sessions` contains exact IDs, workspaces, runtime evidence and connection details. Discovery never registers a peer, sends a message, or starts a client. `peer list` continues to show only registered addresses.

Workspace filters and recipient checks compare resolved paths, so a native session opened through a symbolic link matches its registered directory. Endpoint options must agree with `--harness` when it is supplied.

| Source | What its runtime status means |
| --- | --- |
| Claude `agents --json` | `running` requires a live native record and matching session, workspace and owned socket. It does not say whether the model is busy. |
| Codex app-server | `idle` or `active` comes from the server's loaded threads. `systemError` reports a loaded thread with a runtime problem. |
| Codex writer locks | `writer_active` means a native CLI thread holds its writer lock in the Linux kernel. It does not establish a running model turn or queue consumption. |
| OpenCode server | `idle`, `busy` or `retry` is the addressed server's state. An idle session need not have an attached TUI. |
| OpenCode saved metadata | `unknown` means only that an unarchived session was saved. Supply its running server URL before sending. |

Codex checks its default app-server socket and native writer locks. `--codex-socket` selects explicit servers instead. Writer discovery reads `/proc/locks` and exact SQLite address rows; it never takes a lock. Stale lock files alone are not evidence of a live session. OpenCode checks `http://127.0.0.1:4096` by default and reads recent saved session addresses. Repeat either endpoint option to inspect several servers.

`sources` reports `ok`, `partial` or `unavailable` for each source. Empty results cover only the listed sources and the current execution environment. Sandboxes, process namespaces, stopped servers and custom client homes can limit discovery. Inspect the source diagnostics before concluding that no session exists. Addresses are a snapshot; `peer check` and notification preflight verify them again.

## Address the participants

Use the exact session ID and workspace returned by discovery or the native client. Codex and Claude IDs are UUIDs; OpenCode IDs start with `ses`. Ordinary Codex TUI sessions need no socket argument. For a discovered app-server address, retain its `--socket PATH`.

```sh
htalk peer add builder --harness codex --session CODEX_UUID \
  --workspace /absolute/builder
htalk peer add reviewer --harness claude --session CLAUDE_UUID \
  --workspace /absolute/reviewer
htalk peer add helper --harness opencode --session ses_EXACT_ID \
  --workspace /absolute/helper --url http://127.0.0.1:4096
htalk peer check reviewer
htalk peer check builder
htalk peer list
```

Names and session addresses are immutable. `peer add` only records an address. `peer check` inspects available identity evidence. Notification repeats this identity check before its single attempt. The ordinary Codex check reads the exact saved UUID, workspace, source and archive state from local client metadata; it reports runtime readiness as unknown. A queued notification can be consumed by the existing TUI. An unavailable client can still read saved messages through `inbox`.

Before the first send, run `peer check` in the same execution scope that will send the message. `recipient_unavailable` can mean discovery is restricted; it does not prove that the client is offline. If a known live Claude session is invisible, use the client's normal permission approval for the specific check and send commands. Do not change global permissions or replay a saved notification. Retrieve an already-saved message through `inbox`, `show`, or `wait`.

## Ask, answer, and recover

In the builder session:

```sh
export HTALK_PEER=builder
htalk send reviewer --message 'Which contract needs another test?' --wait 45
htalk wait REQUEST_UUID --seconds 45
htalk inbox
```

In the reviewer session, using the same database:

```sh
export HTALK_PEER=reviewer
htalk inbox
htalk ack REQUEST_UUID
htalk reply REQUEST_UUID --message 'Test retrieval after a lost notification.'
htalk send builder --message 'Can you confirm the fix?'
```

`--as NAME` overrides `HTALK_PEER`. `--message-file PATH` avoids quoting multiline bodies. `--no-notify` saves for polling only. `show MESSAGE_UUID` retrieves one message, including its correlated answer. `sent` recovers outgoing IDs when output or waiting was interrupted.

A reply is a separate message addressed back to the request's sender. Read the answer, then `ack ANSWER_UUID`. Reading never marks anything read. A question can be closed only by replying, including a short decline. An acknowledged question remains in the inbox until answered; an acknowledged answer leaves the inbox. An identical reply retry returns the existing answer and sends no notification. A different answer is rejected and preserves the first.

For retryable automation, generate a UUID before calling `send`, pass `--id UUID`, and retain it. An identical retry returns the saved message without another notification. A reused UUID with different contents is rejected. Without a retained ID, use `sent` after an interrupted send rather than sending again.

Message output includes a `recovery` object with executable commands containing the database path, peer name and full message IDs. A pending request offers `show` and `wait`. A received answer offers `show_reply` and `ack_after_reading`; run the acknowledgment only after reading. `inbox` and `sent` include these commands on each message. They remain usable when a notification failed or the sender process has ended. Interrupted output includes the saved or caller-supplied ID when available and commands to recover outgoing and incoming IDs. Its `persistence` is `saved` only after a save returned; otherwise it is `unknown`, so inspect the database before deciding what happened.

If a Codex session UUID conflicts with a registered peer, the error names both sessions. Its `recovery.peers` command lists the immutable addresses. Select a peer registered to the current session, or return to the named original session before using its inbox. Register a different peer name for a separate session; an existing peer cannot be reassigned.

For evidence in a reply, use an ordinary message file:

```text
Result: The recovery regression passed.
Artifact: /absolute/shared/path/test-output.txt
Commit: <full commit SHA>
Validation: <command run and observed result>
```

Send it with `htalk reply REQUEST_UUID --message-file result.txt`. This records the sender's report; the recipient still needs to inspect the artifact before treating it as verified evidence.

## Observable states

Every message has a durable `id`, monotonic arrival `seq`, sender, recipient, optional `in_reply_to`, body and timestamps. Notification has its own `submission`, detail and attempt timestamps:

| Submission | What is known |
| --- | --- |
| `not_submitted` | Notification was disabled, never attempted, or identity validation failed before transport. |
| `submission_unknown` | Notification was claimed for one attempt, but its final outcome is uncertain. |
| `submitted` | Claude socket bytes were written, Codex acknowledged a queue entry, or OpenCode accepted `prompt_async`. |

None proves model receipt. `ack_at` records the recipient's explicit acknowledgment. A stored answer gives the request `state: reply_received`, independently of notification outcome. Waiting only polls the database for 0–45 seconds and can be resumed after timeout or interruption. It does not resend, invoke models, or acknowledge answers.

The database is committed before client I/O. An interruption during notification leaves an uncertain result. There is no notification retry command and no automatic replay, including on identical `send --id` or `reply` retries. Recovery is through the durable inbox.

Commands print JSON. Exit 0 means the local operation succeeded; exit 2 means invalid input, all discovery sources unavailable, or a notification attempted by this invocation without a confirmed submission. Retrieval, acknowledgment, and identical retries return 0 even when the original notification failed. The message may already be saved on exit 2: inspect its ID and `submission`. Explicit `--no-notify` succeeds with exit 0. Ctrl-C returns 130 and recovery guidance.

## Trust and limits

This is a shared local tool for mutually trusted processes under one OS account. Names and `--as` are routing assertions, not authenticated identities. For a Codex actor, the CLI rejects a conflicting `CODEX_THREAD_ID` when available. Other launchers can inherit that variable, so it is ignored for Claude and OpenCode actors. Live client evidence verifies the addressed recipient, not who invoked the shell command. Anyone with database access can read or change it directly.

Peer contents never grant owner authorization. Notifications contain an inbox command and message ID, without interpolating the message body into client input. Follow each session's existing instructions when deciding whether to act on a peer request. `htalk` neither changes those instructions nor grants filesystem access.

htalk does not launch clients, create sessions, run a polling daemon, or set up accounts. An accepted OpenCode notification can start a model turn in the existing addressed session, using that session's settings. There is no Boardmail dependency or remote-host transport. See [adapter notes](docs/adapters.md) and [OpenCode notes](docs/opencode.md) for connection and wakeup behavior.

## Verify

From a source checkout:

```sh
python3 -m pip install .
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

## Contributions and releases

Open an [issue](https://github.com/jointsome0-lgtm/harness-talk/issues) for bugs, feature requests, adapter needs, or proposed fixes. We do not accept external pull requests. Personal forks and modifications are welcome under the [MIT License](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) and the [release procedure](docs/releasing.md).
