# What works on which system

As of 0.14.0. A cell says whether the part is ported and what stands behind the claim. "Runner" is a test on a GitHub-hosted runner of that system, run on every pull request by `tests.yml` and on every wheel by `publish.yml`. "Live" is a dated check with real clients on a person's machine. A runner is not a person's machine: no real Codex, Claude Code or OpenCode has run on macOS or Windows.

A part that is not ported answers `unsupported_on_this_platform` with exit code 2 and writes nothing. A notice to a peer whose delivery is not ported is recorded as not submitted with that code, and the message stays saved.

| Part | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Wheel | x86-64 and ARM64, glibc 2.28+. Runner, and live for x86-64. | ARM64 (macOS 11+) and x86-64 (10.12+). Runner: each wheel installed and run on its own architecture. | x64. Runner. No ARM64 wheel. |
| Source install | Runner: the source archive is rebuilt. | Runner: `pip install .` from the checkout. | Runner: `pip install .` from the checkout. |
| Mailbox, pull peers, `send`, `reply`, `wait`, `show`, `ack`, `inbox`, `sent`, schema upgrades, Ctrl-C with exit 130 | Runner (`test_cli_compat`, `test_mail_recovery`) and live. | Runner: the same tests, and `tests/platform.rs`. | Runner: the same tests, and `tests/platform.rs`. |
| Two writers at once | Runner (`ContendedWrites`). | Runner (`ContendedWrites`). | Runner: one of three `ContendedWrites` tests; two need a named pipe in the file tree and are skipped. |
| `watch` | Runner and live. | Runner (`InboxWatch`). | Runner (`InboxWatch`). It does not notice that its reader has gone until its next line. |
| The MCP tool, `htalk mcp` | Runner (`test_mcp`) and live. | Runner (`test_mcp`, `tests/process_group.rs`). | Runner (`test_mcp`). |
| `htalk mcp --connect` | Runner (`test_mcp`). | Runner (`test_mcp`). | Runner (`test_mcp`). A process the connector starts in its first moment can outlive it. |
| OpenCode delivery and discovery | Runner and live, see [OpenCode](opencode.md). | Runner, against the test server only (`OpenCodeServer`, `OpenCodeDiscovery`). | Runner, against the test server only. |
| Native delivery to Codex and Claude Code, their discovery, `receive` | Runner and live, see [adapters](adapters.md). | `unsupported_on_this_platform`. Runner: `tests/platform.rs`. | `unsupported_on_this_platform`. Runner: `tests/platform.rs`. |
| The catalogue of profiles | Runner (`test_catalog`, `test_catalog_channels`). | `unsupported_on_this_platform`. Runner: `tests/platform.rs`. | `unsupported_on_this_platform`. Runner: `tests/platform.rs`. |
| Session receivers and managed sessions of [integrations](../integrations/README.md) | Runner; live as each page says. | Nothing backs it. They read `watch`, which is ported; none was run there. | Nothing backs it. |

On the runners 87 contract tests run on each of the two systems. macOS skips 29 tests and subtests, all of native delivery. Windows skips 31: the same 29 and the two that need a named pipe. Every skip prints its reason.

## The default mailbox

`--db PATH`, then `HTALK_DB`, then `$XDG_DATA_HOME/harness-talk/mail.sqlite3` when the variable is set and not empty, on every system. Then:

| System | Path |
| --- | --- |
| Linux | `~/.local/share/harness-talk/mail.sqlite3` |
| macOS | `~/Library/Application Support/harness-talk/mail.sqlite3` |
| Windows | `%LOCALAPPDATA%\harness-talk\mail.sqlite3` |

The file is schema 3 on every system and holds nothing that one system writes and another does not. Sharing one file between two systems over a network drive was not checked.

## Limits to know

- macOS: SQLite syncs with the ordinary call of the system, which does not make the drive write its cache. `PRAGMA fullfsync` is not switched on, so a message saved a moment before a power loss can be missing afterwards. A crash of the program or of the system without a power loss loses nothing.
- macOS: the runner took about 19 seconds for a writer that gives up after its 5-second budget when another holds the mailbox. Whether a real Mac is that slow is not known.
- Windows: the default mailbox is private because the profile directory is. htalk sets no access list. A `--db` somewhere else is as private as its directory.
- Windows: Ctrl-C and Ctrl-Break end a command with exit 130. The MCP server ends a cancelled tool call at once, with no grace period, and ends its tool calls when it is killed itself.
- Windows: a workspace is saved as `C:\dir`. A path longer than 260 characters is not supported as a workspace.
- WSL is Linux and is not what this page means by Windows.
