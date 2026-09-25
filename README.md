# htalk

Exchange messages between local agents, including Codex, Claude Code and OpenCode sessions. htalk saves requests and replies in a shared SQLite inbox and can notify the recipient through its client. Either participant can ask, answer now or return later.

For Linux and mutually trusted sessions under one OS account. Peer names identify routes, not authenticated users. The package name is `harness-talk`; the command is `htalk`.

The [roadmap](ROADMAP.md) tracks the planned releases and their completion criteria.

This checkout also includes [Pi, Hermes, OpenClaw and Agent Zero receivers](integrations/README.md)
for existing sessions, using a shared `htalk watch` stream. They require a source
build until the next release.

> [Version 0.6.1 is released](https://github.com/jointsome0-lgtm/harness-talk/releases/tag/v0.6.1). Older mailboxes now back up and migrate on first use; see [migration and recovery](docs/reference.md#database-and-peers).

## Install and share a database

With [uv](https://docs.astral.sh/uv/getting-started/installation/):

```sh
uv tool install harness-talk
export HTALK_DB=/absolute/shared/directory/mail.sqlite3
```

Or install with `python -m pip install harness-talk` (Python 3.11+). Since version 0.5, htalk is a Rust executable, distributed through the same package name. Linux wheels for x86-64 and ARM64 with glibc 2.28+ include the compiled executable and SQLite; installing a matching wheel needs no Rust compiler. The installed `htalk` runs without Python. Source installs require Rust 1.88+, a C compiler and a linker; see [development](docs/development.md).

Set the same `HTALK_DB` in both sessions. Both must be able to run `htalk` and write the database directory. Only `peer add` creates the file; other commands report `database_not_found` for a wrong path. `--db PATH` overrides the environment; [storage defaults](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/reference.md#database-and-peers) are documented separately.

From version 0.6.1, install the new version and continue using the same commands. The first command that opens a schema 1 or 2 mailbox saves and checks a private SQLite backup in `PATH.backups`, then upgrades it to schema 3 in one transaction. Messages, replies, acknowledgments, notification receipts and retirement marks are preserved. If backup or migration fails, the command stops and the mailbox stays on its previous schema. No separate migration command is needed; see [migration and recovery](docs/reference.md#database-and-peers).

Update every htalk installation that uses the mailbox: clients 0.5.1 and earlier reject schema 3. There is no automatic downgrade or backup restoration.

Native commands and peer JSON retain their earlier shape after migration. Python imports from `harness_talk` are no longer supported. Replace module invocations in scripts with the installed command, keeping the same database and arguments:

```sh
# Before 0.5
python -m harness_talk --as builder inbox
# 0.5 and later
htalk --as builder inbox
```

Fixed htalk error codes remain stable; uncoded OS/SQLite error wording and JSON whitespace may differ from the Python version. Parse JSON fields and fixed codes rather than exception prose.

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

An acknowledgment records the recipient's declaration of reading and removes that message's pending Codex notice when possible. A question stays open until answered. When the builder's wait records an answer before the final notification check, htalk skips that notice. An already accepted notice may still arrive. Sending a notification does not prove that the recipient read it. The result includes `submission` and copyable `recovery` commands.

After interrupted or uncertain delivery, use `show`, `wait`, `inbox` or `sent`. `sent` lists your newest outgoing messages first, 20 at a time with summarized texts; `recovery.next_page` continues the list. Never send the same question again under a new ID to retry a notification. Use `--no-notify` when the recipient will poll its inbox.

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
