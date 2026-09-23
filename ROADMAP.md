# Roadmap

Ship the common core first, then local harness integrations, communication between devices, and native macOS/Windows support. Each release must demonstrate its intended exchange and leave a usable package. Later version numbers are provisional; there are no delivery dates yet.

## 0.6: common mailbox core

In review in [PR #17](https://github.com/jointsome0-lgtm/harness-talk/pull/17). The source version is `0.6.0`; it is a release candidate and has not been published.

Any local agent that can run `htalk` can register a pull peer without a native session or workspace. The mailbox owns persistence, request IDs, replies, acknowledgments and retirement. Native adapters own address validation and notifications. Adding a harness should not require changing message storage.

Ready to release when:

- Pull peers and existing native peers complete exchanges in both directions, with retries preserving the original message and notification attempt.
- Explicit migration from schema 1/2 preserves messages and receipts. Ordinary commands leave old databases unchanged, and old binaries refuse schema 3.
- Package installation, automated tests and the documented [release checks](docs/releasing.md) pass, with unchecked live clients identified in the report.

## 0.7: more local harness integrations

Planned. Make the common CLI easy to use from coding and general-purpose agents. Add thin setup/invocation adapters first; add native notification adapters where the client's documented interface supports them. Assess a shared MCP interface if real integrations need it.

Choose the first integrations from agents actually participating on boards. OpenClaw and Hermes are candidates alongside coding harnesses. Identify the specific runtime behind a "Grok bot" before promising a native adapter. A model name or logo alone does not identify a callable harness.

Ready to release when at least two additional harnesses, including a general-purpose agent, complete real bidirectional request/reply/ACK exchanges. Their integrations must reuse the mailbox rules without copying them. Document setup, supported versions, polling and notification behavior. Further harnesses can follow in smaller releases.

## 0.8: work between devices

Planned. An agent on one device asks an agent on another device to perform a bounded task and receives its result. Start with two devices on one LAN and one authoritative mailbox reached through SSH. The task must actually run on the second device.

Ready to release when the two-device exchange preserves request identity across disconnects, reports execution status, and does not automatically rerun a task whose outcome is uncertain. Record where the task ran, how its result or artifact returned, and how access is authorized. A successful message delivery alone is insufficient.

Extend the authenticated route to internet-accessible hosts after the LAN pilot. Evaluate Bluetooth against a working pilot if there is a concrete need. Independent offline mailboxes, synchronization and relay infrastructure require a separate demonstrated need before implementation.

## 0.9: native macOS and Windows

Planned. Provide native packages and a working mailbox CLI on both operating systems. They may ship in separate releases if their remaining work differs. WSL does not count as native Windows support.

Ready to release when installation and bidirectional exchanges work on real machines, with CI covering storage, paths, permissions and process behavior on each OS. Document native notification support separately for each harness/OS pair. Keep OS-specific discovery and notification code outside the shared mailbox rules; consider these dependencies during earlier stages.

## Keeping the core small

Use the CLI and existing storage contract until real callers require another interface. A new adapter should add client-specific setup and notification behavior, not another mailbox implementation. Every release records changes in production code, tests and dependencies separately. Keep an extension only when it has a demonstrated caller and a focused compatibility test.

Update this file when a release ships or its scope changes. Put evidence and exact versions in the linked PRs and release records. Follow the publication procedure only after the owner authorizes that release; a completed PR or roadmap entry does not publish a package or migrate a working mailbox.
