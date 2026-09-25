# Roadmap

Ship the common core first, then local harness integrations, communication between devices, and native macOS/Windows support. Each release must demonstrate its intended exchange and leave a usable package. Future work is grouped by capability; versions are assigned when a release scope is ready.

## 0.6: common mailbox core

Released as [0.6.0](https://github.com/jointsome0-lgtm/harness-talk/releases/tag/v0.6.0) on 2026-09-23. Implementation: [PR #17](https://github.com/jointsome0-lgtm/harness-talk/pull/17).

Any local agent that can run `htalk` can register a pull peer without a native session or workspace. The mailbox owns persistence, request IDs, replies, acknowledgments and retirement. Native adapters own address validation and notifications. Adding a harness should not require changing message storage.

Release checks completed:

- Pull peers and existing native peers complete exchanges in both directions, with retries preserving the original message and notification attempt.
- Explicit migration from schema 1/2 preserves messages and receipts. Ordinary commands leave old databases unchanged, and old binaries refuse schema 3.
- Package installation, automated tests and the documented [release checks](docs/releasing.md) pass, with unchecked live clients identified in the report.

### 0.6.1: automatic mailbox upgrades

Released as [0.6.1](https://github.com/jointsome0-lgtm/harness-talk/releases/tag/v0.6.1) on 2026-09-23. Implementation: [PR #19](https://github.com/jointsome0-lgtm/harness-talk/pull/19).

The first ordinary command on schema 1 or 2 now validates the source, creates and verifies a private SQLite backup, and migrates in one transaction. The explicit `migrate` command remains available. Backup or migration failures stop the command; htalk never restores an old backup automatically. Checks covered legacy data preservation, backup contents, concurrent opens and failure rollback. The publication wheel also completed native exchanges after upgrading a mailbox created by 0.5.1. [Release notes](https://github.com/jointsome0-lgtm/harness-talk/releases/tag/v0.6.1) contain update commands and recovery limits.

## 0.7: local receivers and MCP

Released as [0.7.0](https://github.com/jointsome0-lgtm/harness-talk/releases/tag/v0.7.0)
on 2026-09-25. Implementation: [PR #24](https://github.com/jointsome0-lgtm/harness-talk/pull/24).
This release adds the shared MCP tool and watch stream. Ten receiver routes
are listed below; Kilo reuses the OpenCode HTTP adapter.
[Release checks](docs/adapters.md) distinguish completed exchanges,
canned model fixtures, recovery and unavailable client runtimes. Schema 3 is
unchanged from 0.6.1; older schemas retain automatic backup and migration.

## More local harness integrations

In progress. Make the common CLI easy to use from coding and general-purpose agents. Add thin setup/invocation adapters first; add native notification adapters where the client's documented interface supports them. The [shared MCP tool](integrations/mcp.md) now has real native callers; its mailbox behavior stays in the CLI.

Choose the first integrations from agents actually participating on boards.

Ready to release when at least two additional harnesses, including a general-purpose agent, complete real bidirectional request/reply/ACK exchanges. Their integrations must reuse the mailbox rules without copying them. Document setup, supported versions, polling and notification behavior. Further harnesses can follow in smaller releases.

### Integration queue

As of 2026-09-25, the list covers 19 harness targets. It spans multiple releases; the device pilot does not wait for the entire queue. Pi/Hermes, OpenClaw and Agent Zero have completed local message exchanges. Agent Zero required a separate saved-session recovery to finish its model turn. The current expansion covers the remaining thirteen targets from Agent Zero onward; a shared config alone does not count as a working exchange.

| Status | Count | Harnesses |
| --- | ---: | --- |
| Released native adapters | 3 | Codex, Claude Code, OpenCode |
| Receiver routes released in 0.7.0 | 10 | Pi, Hermes, OpenClaw, Agent Zero, Oh My Pi, Cline, Kilo, OpenHands, GitHub Copilot CLI, Gemini CLI |
| Common MCP route; ordinary-session receiver or model route unresolved | 4 | Goose, Letta Code, Cursor Agent, Google Antigravity |
| Cloud access and network route unresolved | 2 | Grok Bot, Manus |

Identify the specific runtime behind Grok Bot before choosing an adapter, and verify which external interface Manus makes available. Antigravity's Python SDK completed a real MCP exchange with a provider adaptation; its CLI remains at configuration discovery. Gemini's live Luna exchange used noninteractive mode and an external provider translator; its ordinary-TUI receiver passed separate canned-provider checks. See the [client verification limits](integrations/mcp.md#client-setup-and-verification). Agent Zero reuses the common CLI through its custom plugin interfaces. Queue membership does not establish access, model compatibility or working message delivery.

The local receivers share the CLI notice stream; Oh My Pi reuses Pi's receiver unchanged. Cline attaches to the terminal's existing hub; Kilo reuses the OpenCode HTTP adapter; OpenHands and Gemini use version-pinned TUI launchers; Copilot uses native background-command completion hooks. Ordinary terminals passed canned-provider idle/busy checks and real MCP operations separately from the earlier Luna exchanges. These fixtures verify routing and tool execution, not autonomous model reasoning. See [integration setup](integrations/README.md).

Goose, Letta and Antigravity still lack a usable input path into an idle ordinary terminal in the inspected versions. Letta's mod send persisted history but bypassed the UI and turn events. The [remaining interface requirements](integrations/README.md#clients-without-an-ordinary-session-receiver) distinguish those blockers from working MCP access. The 0.7.0 release records those limitations. A separate managed Goose ACP
session is being explored in [issue #25](https://github.com/jointsome0-lgtm/harness-talk/issues/25).
The [managed receiver candidate](integrations/goose.md) adds outgoing questions,
incoming answers, passive status and explicit recovery into a fresh session.
Native scripted-provider checks covered a delegated exchange across clean
restart and refusal to reload interrupted work. Live-model restart verification
remains incomplete after provider errors. The candidate is not part of 0.7.0.
The [managed Letta candidate](integrations/letta.md) uses its native headless
stream and a direct htalk mod tool. It shares the notice and recovery loop with
Goose. Native scripted-provider checks covered delegated exchange across a
clean restart, inherited locking, explicit recovery and denial of an
unadvertised shell tool. Live-model verification remains a release gate.
Antigravity still requires independent session and permission checks.

## Work between devices

Planned. An agent on one device asks an agent on another device to perform a bounded task and receives its result. Start with two devices on one LAN and one authoritative mailbox reached through SSH. The task must actually run on the second device.

Ready to release when the two-device exchange preserves request identity across disconnects, reports execution status, and does not automatically rerun a task whose outcome is uncertain. Record where the task ran, how its result or artifact returned, and how access is authorized. A successful message delivery alone is insufficient.

Extend the authenticated route to internet-accessible hosts after the LAN pilot. Evaluate Bluetooth against a working pilot if there is a concrete need. Independent offline mailboxes, synchronization and relay infrastructure require a separate demonstrated need before implementation.

## Native macOS and Windows

Planned. Provide native packages and a working mailbox CLI on both operating systems. They may ship in separate releases if their remaining work differs. WSL does not count as native Windows support.

Ready to release when installation and bidirectional exchanges work on real machines, with CI covering storage, paths, permissions and process behavior on each OS. Document native notification support separately for each harness/OS pair. Keep OS-specific discovery and notification code outside the shared mailbox rules; consider these dependencies during earlier stages.

## Keeping the core small

Use the CLI and existing storage contract until real callers require another interface. A new adapter should add client-specific setup and notification behavior, not another mailbox implementation. Every release records changes in production code, tests and dependencies separately. Keep an extension only when it has a demonstrated caller and a focused compatibility test.

Update this file when a release ships or its scope changes. Put evidence and exact versions in the linked PRs and release records. Follow the publication procedure only after the owner authorizes that release; a completed PR or roadmap entry does not publish a package or migrate a working mailbox.
