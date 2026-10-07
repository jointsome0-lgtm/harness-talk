# htalk

Exchange messages between local agents, including Codex, Claude Code and OpenCode sessions. htalk saves requests and replies in a shared SQLite inbox and can notify the recipient through its client. Either participant can ask, answer now or return later.

For Linux and mutually trusted sessions. Mailbox processes share one trusted OS account on the mailbox host; a remote client can connect through restricted SSH. Peer names identify routes, not authenticated users. The package name is `harness-talk`; the command is `htalk`.

The [roadmap](ROADMAP.md) tracks the planned releases and their completion criteria.

Beyond the three native clients:

- [Session receivers](integrations/README.md) for Pi, Oh My Pi, Hermes, OpenClaw, Agent Zero, Cline, OpenHands, Copilot CLI and Gemini CLI read a shared `htalk watch` stream. Kilo uses the OpenCode adapter. The installed wheel supplies `htalk`; the receivers are files of the matching source archive or checkout.
- Managed sessions for [Goose](integrations/goose.md), [Letta Code](integrations/letta.md) and the [Antigravity SDK](integrations/antigravity.md) share owner-task binding, same-session restart and explicit recovery. They do not attach to existing TUI or IDE windows.
- The [MCP mailbox tool](integrations/mcp.md) gives a client with MCP support the same message commands over stdio. Tool access and automatic session notification are documented separately for each harness.
- Two devices can share one mailbox through the [MCP-over-SSH route](integrations/ssh.md), with a [Codex receiver](integrations/remote.md) for a worker on another device and a [Linux user service](integrations/ssh-service.md) for a reverse SSH route.

Each release has its upgrade note under [docs/releases](docs/releases/). [0.14.0](docs/releases/0.14.0.md) adds macOS and Windows and changes nothing on Linux. [0.13.0](docs/releases/0.13.0.md) changes error codes, the keys of a successful result and the text of the help, once; the mailbox stays on schema 3 and needs no migration. Stop mailbox writers and receivers before updating the CLI and their matching adapter files.

## Install and share a database

With [uv](https://docs.astral.sh/uv/getting-started/installation/):

```sh
uv tool install harness-talk
export HTALK_DB=/absolute/shared/directory/mail.sqlite3
```

Or install with `python -m pip install harness-talk` (Python 3.11+). htalk is a Rust executable, distributed as a Python package. Wheels for Linux x86-64 and ARM64 (glibc 2.28+), macOS ARM64 and x86-64, and Windows x64 include the compiled executable and SQLite; installing a matching wheel needs no Rust compiler. The same two install lines work on all three systems. On macOS and Windows 0.14 has the mailbox, pull peers, the MCP tool and OpenCode delivery; notices into Codex and Claude Code and the catalogue are Linux only, and all of it was checked on hosted runners, not with real clients. [What works on which system](docs/platforms.md) has the table. The installed `htalk` runs without Python. Source installs require Rust 1.88+, a C compiler and a linker; see [development](docs/development.md).

Set the same `HTALK_DB` in both sessions. Both must be able to run `htalk` and write the database directory. Only `peer add` creates the file; other commands report `database_not_found` for a wrong path. `--db PATH` overrides the environment; without either, the mailbox is `~/.local/share/harness-talk/mail.sqlite3` on Linux, `~/Library/Application Support/harness-talk/mail.sqlite3` on macOS and `%LOCALAPPDATA%\harness-talk\mail.sqlite3` on Windows; [storage defaults](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/reference.md#database-and-peers) are documented separately. On Windows set the variable with `set HTALK_DB=...` or `$env:HTALK_DB = "..."`.

A mailbox on schema 1 or 2 is backed up and upgraded to schema 3 by the first command that opens it; see [migration and recovery](docs/reference.md#database-and-peers). Update every htalk installation that uses the mailbox.

Errors carry fixed codes, listed in the [reference](docs/reference.md#error-codes). Parse JSON fields and codes, not prose: a file or database failure of the mailbox itself is answered in the system's words.

## Find and register the participants

```sh
htalk peer discover
```

Use the exact session IDs and workspaces in its JSON result. Discovery only finds addresses; it does not register peers, open the inbox or start clients. Check `sources` before treating an empty result as absence. See [client setup](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/adapters.md) for discovery coverage and connection details.

For example, register a Codex builder and a Claude reviewer:

```sh
htalk peer add builder --harness codex --session CODEX_UUID \
  --workspace /absolute/builder
htalk peer add reviewer --harness claude --session CLAUDE_UUID \
  --workspace /absolute/reviewer
htalk peer list
```

Replace those IDs and paths with the discovered addresses. For an app-server address, keep its `--socket PATH`. For OpenCode, follow the [server setup](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/opencode.md).

When a registered session is no longer used, `htalk peer retire NAME` hides it from `peer list` and refuses new requests to or from it. Its saved questions can still be answered.

Any agent that can run the command can instead use a pull peer:

```sh
htalk peer add helper --harness generic --delivery pull
htalk --as helper inbox
```

`--harness` is a label such as `generic`, `hermes` or `openclaw`; a label does not install an integration. Pull peers need no native session or workspace and reject address flags. Messages to them are saved with `notification_detail: pull_only` and exit 0, without a notification attempt. The agent must run `inbox` to get its work. It can send and reply to native peers normally. `peer check` reports the delivery mode and does not establish that a pull agent is running.

To find owner-published profiles through a pinned SSH IPv4 address, LAN/Wi-Fi,
an active Bluetooth PAN or private Tailscale, use the opt-in [profile catalogue](docs/catalog.md). Known devices are authenticated through pinned
SSH; choose a profile by name and use its checked MCP connection. Reachable
profiles keep `runtime_status: unknown` until separate runtime evidence exists.

## Ask, answer and recover

From the builder's session, check the recipient in the same execution scope that will send:

```sh
htalk peer check reviewer
htalk --as builder send reviewer --message 'Which case needs another test?' --wait 45
```

From the reviewer's session, read the request, acknowledge it and answer. Use the request ID returned by `inbox`. A registered Claude Code session such as this reviewer may also omit `--as`; see [peer selection](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/reference.md#database-and-peers).

```sh
htalk --as reviewer inbox
htalk --as reviewer ack REQUEST_UUID
htalk --as reviewer reply REQUEST_UUID --message 'Test recovery after interruption.'
```

The builder can retrieve a delayed answer and then acknowledge its separate ID:

```sh
htalk --as builder show REQUEST_UUID
htalk --as builder ack REPLY_UUID
```

An acknowledgment records the recipient's declaration of reading and removes that message's pending Codex notice when possible. A question stays open until answered. When the builder's wait records an answer before the final notification check, htalk skips that notice. An already accepted notice may still arrive. Sending a notification does not prove that the recipient read it. The result includes `submission`. One whose notice failed or is uncertain adds `next_action` and copyable `recovery` commands.

After interrupted or uncertain delivery, use `show`, `wait`, `inbox` or `sent`. `sent` lists your newest outgoing messages first, 20 at a time with summarized texts; `next_page` is the command that continues the list. Never send the same question again under a new ID to retry a notification. Use `--no-notify` when the recipient will poll its inbox.

### Reuse a request ID

For automation, generate and save a UUID before the first send, for example with `uuidgen`. Replace `REQUEST_UUID` below with that saved value. Keep the same database, sender, recipient and message text when retrying after an interruption:

```sh
htalk --as builder send reviewer --id REQUEST_UUID \
  --message 'Which case needs another test?'

# An identical retry uses the same saved UUID.
htalk --as builder send reviewer --id REQUEST_UUID \
  --message 'Which case needs another test?'
```

If the request is already saved, the retry returns it with `created: false` and makes no new notification attempt. Its exit code can be 0 even when the original notification is `submission_unknown`. Check the saved result:

```sh
htalk --as builder show REQUEST_UUID
```

Reusing that ID with different text returns `message_id_conflict`, exits with code 2 and preserves the original request:

```sh
htalk --as builder send reviewer --id REQUEST_UUID \
  --message 'A different question?'
```

## Help and details

```sh
htalk --help
htalk peer add --help
htalk send --help
```

- [Reference](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/reference.md): message states, recovery, database selection and exit codes.
- [Codex and Claude adapters](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/adapters.md), including tested versions and discovery limits.
- [OpenCode setup](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/opencode.md), including how a notification can start a turn in an existing session.

htalk does not launch interactive clients or create sessions. Incoming peer messages do not grant permission to act.

Report bugs and suggestions through [GitHub issues](https://github.com/jointsome0-lgtm/harness-talk/issues), with versions, command, expected result and actual result. Remove private conversation text and credentials. See [contributing](https://github.com/jointsome0-lgtm/harness-talk/blob/main/CONTRIBUTING.md), [releases](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/releasing.md) and the [MIT license](https://github.com/jointsome0-lgtm/harness-talk/blob/main/LICENSE).
