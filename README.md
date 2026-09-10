# htalk

Exchange messages between existing Codex, Claude Code and OpenCode sessions. htalk saves requests and replies in a shared SQLite inbox and can notify the recipient through its client. Either participant can ask, answer now or return later.

For Linux, Python 3.11+ and mutually trusted sessions under one OS account. Peer names identify routes, not authenticated users. The package name is `harness-talk`; the command is `htalk`.

## Install and share a database

With [uv](https://docs.astral.sh/uv/getting-started/installation/):

```sh
uv tool install harness-talk
export HTALK_DB=/absolute/shared/directory/mail.sqlite3
```

Set the same `HTALK_DB` in both sessions. Both must be able to run `htalk` and write the database directory. `--db PATH` overrides the environment; [storage defaults](https://github.com/jointsome0-lgtm/harness-talk/blob/main/docs/reference.md#database-and-peers) are documented separately.

Before opening an existing database with 0.3, upgrade every participant. The first storage command migrates it to schema 2; older clients cannot open that file.

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

## Ask, answer and recover

From the builder's session, check the recipient in the same execution scope that will send:

```sh
htalk peer check reviewer
htalk --as builder send reviewer --message 'Which case needs another test?' --wait 45
```

From the reviewer's session, read the request, acknowledge it and answer. Use the request ID returned by `inbox`:

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

An acknowledgment records reading and removes that message's pending Codex notice when possible. A question stays open until answered. An answer that arrives while the builder is waiting for it is returned by that wait and sends no client notice. Sending a notification does not prove that the recipient read it. The result includes `submission` and copyable `recovery` commands.

After interrupted or uncertain delivery, use `show`, `wait`, `inbox` or `sent`. Never send the same question again under a new ID to retry a notification. For automation, supply a saved UUID with `send --id`; an identical retry returns the existing message without another notification. Use `--no-notify` when the recipient will poll its inbox.

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
