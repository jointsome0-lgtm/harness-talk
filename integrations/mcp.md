# MCP mailbox tool

`htalk mcp` exposes the existing mailbox through one local stdio tool. Use it
when a harness supports MCP and its normal shell is unavailable or inconvenient.
It uses the same executable, database, IDs, replies and ACKs as the CLI.
It does not start an idle model turn. A [session receiver](README.md) provides
that separately where the harness supports it.

This command is included in htalk 0.7.0 and newer, including the binary wheels.

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
| Oh My Pi 18.3.0 | Project `mcp.json` or `.mcp.json`, shape above | Four acknowledged messages exchanged with Pi on Luna/Flex in RPC mode; an ordinary terminal separately passed an idle/busy notice exchange using a local canned provider |
| Cline 3.0.65 | `cline mcp install htalk --transport stdio -- /absolute/path/to/htalk --db /absolute/mail.sqlite3 --as cline-worker mcp` | [Hub receiver](README.md#cline) passed idle/busy native TUI MCP exchanges with a canned provider and a separate normal-exit check. The prior Luna/Flex exchange used Core 0.0.86 SDK |
| Kilo 7.7.9 | `kilo.json`, shape below | [Ordinary attached TUI](README.md#kilo) passed queued delivery and native MCP handling of two saved requests with a canned provider. A separate same-session Luna/Flex exchange completed earlier |
| Goose 1.52.0 | Stdio extension, command below | Retained native ACP session woke from a watch notice and completed show/ACK/reply on Luna/Flex |
| Letta Code 0.33.0 | Local agent MCP server settings, shape below | Native headless session completed show/ACK/reply on Luna/Flex through Bash and Letta's own MCP CLI. The ordinary TUI mod-send check persisted history but did not display its turn |
| OpenHands CLI 1.16.0 / SDK 1.21.0 | `openhands mcp add htalk --transport stdio /absolute/path/to/htalk -- --db /absolute/mail.sqlite3 --as hands-worker mcp` | [TUI launcher](README.md#openhands) passed idle/busy native MCP exchanges, visible answers and normal exit with a canned provider. A separate SDK conversation completed show/ACK/reply on Luna/Flex |
| Cursor Agent 2026.09.23-86fc751 | `~/.cursor/mcp.json`, shape above | `mcp list-tools htalk` discovered `htalk(args)` |
| GitHub Copilot CLI 1.0.88 | `~/.copilot/mcp-config.json`, shape above | [Notification hooks](README.md#github-copilot-cli) passed idle/busy ordinary-TUI exchanges, rearming and normal exit with a canned provider. A separate headless session completed a request/reply/ACK exchange on Luna/Flex |
| Gemini CLI 0.61.0 | `.gemini/settings.json` or user settings, shape above | [TUI launcher](README.md#gemini-cli) passed idle/busy native MCP exchanges, draft preservation and normal exit with a canned provider. A separate noninteractive CLI completed show/ACK/reply on Luna/Flex through an external provider translator |
| Google Antigravity CLI 1.2.10 / Python SDK 0.1.18 | CLI: `~/.gemini/config/mcp_config.json`; SDK: `McpStdioServer`, below | SDK completed show/ACK/reply on Luna/Flex with the provider adaptation described below; the CLI only listed its configuration |

Oh My Pi exposes MCP tools as devices. Write `{"args":[...]}` to
`xd://mcp__htalk_htalk`. Preserve its generated system prompt; a fixed
`--system-prompt` replaces the device instructions. Use `--append-system-prompt`
for additional session instructions. Its [Pi receiver](README.md#oh-my-pi) loads
unchanged with `--hook /absolute/path/to/integrations/pi.ts`.

The recovery resumed the saved session after the initial turn timed out. Its
watch notice started the turn without a separate prompt. OMP reused the original
workspace's MCP configuration, so the reply reached the original mailbox while
the recovery collector waited on a copy and timed out. Independent readback
found the correlated answer there; the controller then acknowledged it.
The exchange completed, but the copied-mailbox fixture did not pass. When
resuming or copying a session, verify its effective MCP configuration and make
the watcher and MCP tool use the same mailbox.

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
it invoked this command through its Bash tool. A native headless session on
Luna/Flex read the saved request, calculated its answer, replied and finished
normally. Both messages were acknowledged. The first attempt had stopped at
an overly strict test permission rule. After correcting that rule, a fresh
local agent handled the same saved request without resending it or importing
the earlier chat. This establishes recovery by mailbox ID, not resumption of
the earlier agent.

A separate ordinary-TUI check used a local canned provider and a Letta mod's
`ctx.conversation.sendMessageStream`. The send persisted in the active
conversation, but neither the input nor the answer appeared in the terminal,
and the mod's turn events did not fire. A later human turn saw that history.
This backend method bypasses the visible TUI turn path; the installed mod guide
also warns against overlapping a busy turn. No ordinary-TUI receiver is included
until the host exposes a queue that preserves that path.

Cursor, Copilot and Antigravity's tested listing commands read user settings;
a project file alone in a plain directory was not sufficient in those checks.
Gemini applies its normal folder-trust policy. Review and trust the intended
project through the client before enabling a local executable there.
Gemini CLI 0.61.0's custom base URL still uses the Gemini API client. The
installed version has no native OpenRouter provider for GPT-6 Luna; changing
the URL and model name alone does not provide that route. The native CLI check
used an external Gemini-to-OpenAI translator based on LiteLLM 1.102.1, with
`GOOGLE_GEMINI_BASE_URL` pointing to its loopback endpoint. Model traffic used
Luna/Flex; MCP used the direct htalk configuration above. The translator was a
test fixture and is not part of htalk or a new package dependency.

Gemini adds a boolean `wait_for_previous` to tool arguments and handles that
ordering hint in its own scheduler before invoking MCP. htalk accepts and
ignores this field; its advertised tool schema remains `args` only. Other
unknown fields and non-boolean values are rejected. This prevents Gemini's
client metadata from blocking otherwise valid mailbox commands.

After this compatibility fix, a new CLI session read and acknowledged the
original saved request, calculated its answer and replied to its ID.
The controller independently read and acknowledged the answer. No request
was resent or old chat imported. The CLI finished normally. A model-free check
also covered the scheduling flag set to true, false and absent. These checks
trusted only disposable workspaces; they do not establish resumption of the
earlier native session. Ordinary idle/busy TUI delivery was checked separately
through the [version-pinned launcher](README.md#gemini-cli) with a canned native
Gemini-protocol endpoint.

The OpenHands check used its installed `Conversation` and `Agent` SDK classes,
the MCP htalk tool and its native finish tool. An external controller forwarded
the notice into the existing idle conversation; the same conversation finished
after replying. Its model traffic used the provider's Responses API. Ordinary
TUI delivery was checked separately through the version-pinned
[receiver launcher](README.md#openhands), using a canned local provider.

For Antigravity's Python SDK, add a stdio server to the existing agent config's
`mcp_servers` list:

```python
from google.antigravity.types import McpStdioServer

htalk = McpStdioServer(
    name="htalk",
    command="/absolute/path/to/htalk",
    args=["--db", "/absolute/mail.sqlite3", "--as", "agy-worker", "mcp"],
    enabled_tools=["htalk"],
)
```

Keep the agent's model configuration and add an appropriate tool policy, such
as `policy.allow(htalk, ["htalk"])` from `google.antigravity.hooks.policy`.
SDK 0.1.18 exposes the tool through `call_mcp_tool`: `ServerName` and `ToolName`
are both `htalk`, and `Arguments` is `{"args":[...]}`. Its wrapper also requires
`toolSummary` and `toolAction`. The SDK supplies that wrapper schema to the
model. A local canned endpoint first verified the native MCP tool calls.

A real `Agent(LocalOpenAIAgentConfig(...))` turn then completed a Luna/Flex
exchange through a bounded local provider gateway. That gateway supplied
OpenRouter authentication and routing. SDK 0.1.18 sent `tool_choice: "none"`
despite listing MCP tools, so the first model turn invoked nothing. Enabling
the SDK's `FINISH` capability did not change that initial request. The gateway
then changed `tool_choice` to `"auto"`, preserving messages and tool schemas.
A fresh SDK session handled the same saved mailbox request, calculated its
answer and completed show/ACK/reply with a normal finish. Both messages were
acknowledged; no request was resent or old chat imported. This verifies the
SDK with that provider adaptation. It does not establish a direct stock
OpenRouter route, CLI/TUI tool execution or automatic wake. The provider
gateway was a test fixture and is not part of htalk.

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
[Antigravity MCP](https://antigravity.google/docs/mcp),
[Antigravity SDK MCP](https://antigravity.google/docs/sdk/mcp/),
[Antigravity SDK local models](https://antigravity.google/docs/sdk/local-models/).
