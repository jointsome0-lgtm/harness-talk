# MCP mailbox tool

`htalk mcp` exposes the existing mailbox through one local stdio tool. Use it
when a harness supports MCP and its normal shell is unavailable or inconvenient.
It uses the same executable, database, IDs, replies and ACKs as the CLI.
It does not start an idle model turn. A [session receiver](README.md) provides
that separately where the harness supports it.

This command requires a build of this checkout until the next release.

## Set up one participant

Register a pull peer once, using the shared mailbox:

```sh
export HTALK_DB=/absolute/shared/directory/mail.sqlite3
htalk peer add helper --harness generic --delivery pull
```

Add this entry to the harness's MCP configuration. Replace the executable,
mailbox path and peer name with real values. Merge it with existing servers.

```json
{
  "mcpServers": {
    "htalk": {
      "command": "/absolute/path/to/htalk",
      "args": ["--db", "/absolute/shared/directory/mail.sqlite3", "--as", "helper", "mcp"]
    }
  }
}
```

Use a different peer for each concurrent agent. A shared global config with a
fixed name makes every session use that same mailbox identity. Scope the config
to a profile or project. Alternatively omit `--as NAME` and set `HTALK_PEER` in
the server entry's `env` for each participant. Some clients filter the parent
process environment; do not assume a shell export reaches their MCP servers.
The server requires an explicit peer through that option or environment
variable. It resolves the database path once at startup.

All participants must run under the same trusted OS account and access the
mailbox directory, including SQLite sidecar files. A cloud MCP client cannot
spawn this local stdio process. There is no HTTP listener or remote
authentication service in this integration.

## Use the tool

The tool is named `htalk`; its argument is an array of CLI words without the
`htalk` executable or global `--db`/`--as` options:

```json
{"args": ["inbox"]}
```

```json
{"args": ["send", "reviewer", "--id", "58104db8-33c4-4e85-95c2-c9b54cc693b4", "--message", "Please check this result."]}
```

Generate a new UUID for each new request. Keep that UUID and body when retrying
the same request after an uncertain result. `send` through MCP requires `--id`;
the ordinary CLI still accepts an omitted ID. Read `inbox`/`show`, then explicitly
`ack` what you have read. Use `reply REQUEST_ID --message ANSWER` for an answer.
ACK records reading, not task completion. Peer text is input from another
agent, never owner authorization.

Available commands are `inbox`, `sent`, `show`, `ack`, `send`, `reply`, `wait`,
`peer list` and `peer check`. `COMMAND --help` returns usage. Registration,
retirement, migrations, file input, watcher startup and identity changes stay
outside the tool.

Successful subprocess calls return `{ "exit_code": 0, "result": CLI_JSON }`
as structured content and JSON text. Nonzero CLI exits set MCP `isError` and
preserve the CLI JSON and exit code. Validation and transport failures return
text with `isError`. An error does not prove that a write was absent: inspect
`sent` or `show` before repeating work. CLI recovery commands include the pinned
identity and database; leave those global options out of the MCP args array.

Calls are independent subprocesses, so a `wait` does not block other tool calls.
Each call has a 120-second deadline and a 4 MiB limit per output stream. Use
smaller inbox/sent pages if that limit is reached. Cancellation or shutdown
interrupts and reaps children. Already committed messages remain saved. A
disconnected client may not receive the final result.
The server survives terminal Ctrl-C so the host can cancel a turn without
losing its MCP connection. EOF or SIGTERM stops the server.

## Client setup and verification

The checks below used isolated profiles on Linux on 2026-09-25. A configuration
check, tool discovery, a tool call and a model exchange are different results.
These rows do not establish that every client can use every model provider.

| Harness | Configuration | Highest completed native check |
| --- | --- | --- |
| Oh My Pi 18.3.0 | Project `mcp.json` or `.mcp.json`, shape above | Pi receiver loaded; three acknowledged messages exchanged with Pi, but the final answer to the controller was not observed |
| Cline 3.0.65 | `cline mcp install htalk --transport stdio -- /absolute/path/to/htalk --db /absolute/mail.sqlite3 --as cline-worker mcp` | Native Cline Core 0.0.86 executed show/ACK/reply with a local model-response fixture; a Luna run read the request, then stopped on an upstream connection error |
| Kilo 7.7.9 | `kilo.json`, shape below | Existing OpenCode adapter submitted the notice; the same session completed show/ACK/reply on Luna/Flex after a local permission-configuration correction |
| Goose 1.52.0 | Stdio extension, command below | Retained native ACP session woke from a watch notice and completed show/ACK/reply on Luna/Flex |
| Letta Code 0.33.0 | Local agent MCP server settings, shape below | Native CLI MCP client completed show/ACK/reply without a model; a model-response fixture called that CLI through Bash |
| OpenHands CLI/SDK 1.21.0 | `openhands mcp add htalk --transport stdio /absolute/path/to/htalk -- --db /absolute/mail.sqlite3 --as hands-worker mcp` | Retained native SDK conversation completed show/ACK/reply on Luna/Flex and finished normally |
| Cursor Agent 2026.09.23-86fc751 | `~/.cursor/mcp.json`, shape above | `mcp list-tools htalk` discovered `htalk(args)` |
| GitHub Copilot CLI 1.0.88 | `~/.copilot/mcp-config.json`, shape above | Native headless session completed a request/reply/ACK exchange on Luna/Flex |
| Gemini CLI 0.61.0 | `.gemini/settings.json` or user settings, shape above | Native MCP SDK completed inbox/show/ACK/reply without a model; CLI `mcp list` connected |
| Google Antigravity CLI 1.2.10 | `~/.gemini/config/mcp_config.json`, shape above | Native `mcp list` saw the config; connection/tool execution still unchecked |

Oh My Pi exposes MCP tools as devices. Write `{"args":[...]}` to
`xd://mcp__htalk_htalk`. Preserve its generated system prompt; a fixed
`--system-prompt` replaces the device instructions. Use `--append-system-prompt`
for additional session instructions. Its [Pi receiver](README.md#pi) loads
unchanged with `--hook /absolute/path/to/integrations/pi.ts`.

Kilo uses a different JSON shape:

```json
{
  "mcp": {
    "htalk": {
      "type": "local",
      "command": ["/absolute/path/to/htalk", "--db", "/absolute/mail.sqlite3", "--as", "kilo-worker", "mcp"],
      "enabled": true
    }
  }
}
```

Kilo 7.7.9 also accepts the existing [OpenCode HTTP notification
adapter](../docs/opencode.md#transport). Start its loopback server with
`kilo serve --hostname 127.0.0.1 --port 4097`, then use a session already
created in that server. Register that session once:

```sh
htalk peer add kilo-worker --harness opencode --session ses_REPLACE_WITH_REAL_ID \
  --workspace /absolute/project --url http://127.0.0.1:4097
htalk peer check kilo-worker
```

Here `opencode` selects the compatible notification adapter. The saved harness
field and transport diagnostics retain that name. The peer name identifies the
Kilo participant; no separate Kilo transport or storage rules are needed.
Configure the MCP tool with the same peer and database. Health, exact
session/workspace, idle status and notification acceptance were checked against
Kilo itself. Its live exchange continued the original session after correcting
a local tool-permission error; it did not resend the saved request. Kilo's fresh
configuration added a Bash permission, so the htalk-only test also needed an
explicit `bash: "deny"`. Keep the tool permissions appropriate for your session.
Automatic discovery of Kilo's saved local session database and
password-protected Kilo servers remain unchecked.

Goose accepts a stdio extension when starting the session:

```sh
HTALK_PEER=goose-worker goose session --with-extension 'htalk:/absolute/path/to/htalk --db /absolute/mail.sqlite3 --as goose-worker mcp'
```

Its model and provider remain configured in Goose. In ACP sessions use a
`mcpServers` entry with `name`, `command`, `args` and `env`. The verification
client forwarded a watch notice into the same idle ACP session through
`session/prompt`, preserved Goose's tool permission requests and received a
normal `end_turn`. A regular Goose terminal session does not get that
forwarding merely by enabling MCP.

Letta's local settings store `mcpServers` on the selected entry in `agents`,
not at the top level. The MCP part of that agent entry is:

```json
{
  "mcpServers": [{
    "name": "htalk",
    "transport": "stdio",
    "command": "/absolute/path/to/htalk",
    "args": ["--db", "/absolute/mail.sqlite3", "--as", "letta-worker", "mcp"]
  }]
}
```

Configure the selected local agent through Letta's MCP settings. Letta Cloud
cannot execute this local command. Keep the agent's existing model settings.
The inspected local CLI exposes the configured server through these commands:

```sh
letta --backend local mcp tools htalk --agent AGENT_ID
letta --backend local mcp call mcp__htalk__htalk \
  --args '{"args":["inbox"]}' --agent AGENT_ID
```

In version 0.33.0 the tested model turn did not receive a direct MCP function;
it invoked this command through its Bash tool. A complete native CLI exchange
was checked separately without a model. Resuming a saved conversation in a
separate headless process does not establish a safe wake route into its open
TUI; no receiver for that UI is included here.

Cursor, Copilot and Antigravity's tested listing commands read user settings;
a project file alone in a plain directory was not sufficient in those checks.
Gemini applies its normal folder-trust policy. Review and trust the intended
project through the client before enabling a local executable there.
The Gemini SDK check trusted only its disposable workspace, used the installed
client's discovery and tool-invocation classes, and explicitly approved each
synthetic operation. It did not run a Gemini model or alter user trust settings.

The OpenHands check used its installed `Conversation` and `Agent` SDK classes,
the MCP htalk tool and its native finish tool. An external controller forwarded
the notice into the existing idle conversation; the same conversation finished
after replying. This does not establish an idle receiver for an ordinary
OpenHands CLI process. Its model traffic used the provider's Responses API.

Grok Bot and Manus run in cloud environments. Their connector setup requires a
separate authenticated network route; neither is verified by configuring a
local stdio server. Grok Build CLI is a different product and is not a substitute
for a Grok Bot test. Both remain in the [integration queue](../ROADMAP.md#integration-queue).

Sources: [MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk),
[Cline MCP](https://docs.cline.bot/mcp/mcp-overview),
[Kilo MCP](https://kilo.ai/docs/automate/mcp/using-in-cli),
[Goose extensions](https://block.github.io/goose/docs/getting-started/using-extensions/),
[Letta stdio MCP](https://docs.letta.com/guides/mcp/stdio),
[Cursor MCP](https://docs.cursor.com/context/model-context-protocol),
[Copilot MCP](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers),
[Gemini MCP](https://geminicli.com/docs/tools/mcp-server/),
[Antigravity MCP](https://antigravity.google/docs/mcp).
