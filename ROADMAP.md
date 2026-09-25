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

## More local harness integrations

In progress. Make the common CLI easy to use from coding and general-purpose agents. Add thin setup/invocation adapters first; add native notification adapters where the client's documented interface supports them. The [shared MCP tool](integrations/mcp.md) now has real native callers; its mailbox behavior stays in the CLI.

Choose the first integrations from agents actually participating on boards.

Ready to release when at least two additional harnesses, including a general-purpose agent, complete real bidirectional request/reply/ACK exchanges. Their integrations must reuse the mailbox rules without copying them. Document setup, supported versions, polling and notification behavior. Further harnesses can follow in smaller releases.

### Integration queue

As of 2026-09-25, the list covers 19 harness targets. It spans multiple releases; the device pilot does not wait for the entire queue. Pi/Hermes, OpenClaw and Agent Zero have completed local message exchanges. Agent Zero required a separate saved-session recovery to finish its model turn. The current expansion covers the remaining thirteen targets from Agent Zero onward; a shared config alone does not count as a working exchange.

| Status | Count | Harnesses |
| --- | ---: | --- |
| Released native adapters | 3 | Codex, Claude Code, OpenCode |
| Implemented locally with live exchanges, not released | 4 | Pi, Hermes, OpenClaw, Agent Zero |
| Common MCP route, native checks and model exchanges in progress | 10 | Oh My Pi, Cline, Kilo, Goose, Letta Code, OpenHands, Cursor Agent, GitHub Copilot CLI, Gemini CLI, Google Antigravity |
| Cloud access and network route unresolved | 2 | Grok Bot, Manus |

Identify the specific runtime behind Grok Bot before choosing an adapter, and verify which external interface Manus makes available. Antigravity's Python SDK completed a real MCP exchange with a provider adaptation; its CLI remains at configuration discovery. Gemini's native noninteractive CLI completed a Luna exchange through an external provider translator; idle terminal wake is unverified. See the [client verification limits](integrations/mcp.md#client-setup-and-verification). Agent Zero reuses the common CLI through its custom plugin interfaces. Queue membership does not establish access, model compatibility or working message delivery.

The local Pi, Hermes, OpenClaw and Agent Zero integrations share the CLI notice stream; see [integration setup](integrations/README.md). Release still requires review of the recorded exchanges and remaining limitations.

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
