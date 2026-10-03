# Client adapters

Checked on 2026-10-03 UTC with the htalk `0.11.1` x86-64 publication wheel,
commit `3c86ebb62151fe77ef5019082269f3ea2020665f`, SHA-256
`0283d9d04398fe884ee59834fa42bc90c4514ac24703df6a353f793d60b9427b`.
The wheel and matching source archive came from
[publication run 37141672055](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/37141672055).
The [0.11.1 native check](checks/2026-10-03-0.11.1-native.md) records exact
artifact hashes, client versions, permissions, publication checks and limits.

All 14 available installed clients completed the mandatory native exchange:
Codex, Claude, Pi, OMP, OpenClaw, Cline, Kilo, Goose, Hermes, Copilot, Gemini,
Letta, OpenHands and OpenCode. Actual Codex/GPT-6.1 Sol high and Claude/Opus 5.5
high checked both initiating directions. The other 12 clients used fresh
synthetic state, fake keys and canned loopback providers without paid inference;
their checks establish native transport and tool execution, not model reasoning.
Every passing case observed a native notice, showed the full request once before
a separate ACK, produced a correlated reply, and ended with the controller's
read and separate reply ACK, empty inboxes and recorded owned process closure.

The mandatory exchange did not establish normal exit for every client. Hermes
and Gemini retain false runner aggregates; Hermes also made an extra rejected
loopback provider request. OMP ended after controller termination. OpenCode
used a headless server without an attached TUI and ended after SIGTERM, exit
`-15`; requested port 0 selected 4096 rather than proving an OS-assigned ephemeral
port. Core closure covers recorded owned processes without a distinct listener
readback. Cleanup remains literal: Codex `absent`, Claude/Kilo/OpenCode
`unsupported`, other adapter exchanges `skipped/pull_only`; none proves notice
removal. Four invalid earlier attempts remain separate, and old mail was not
replayed. Antigravity SDK and Agent Zero remain unchecked. The seven frozen
performance failures remain unresolved. See the [0.11.1 release notes](releases/0.11.1.md)
for upgrade requirements. Earlier dated checks below remain historical evidence.

Checked on 2026-10-03 UTC with the htalk `0.11.0` x86-64 publication wheel,
commit `a8177d79cf53f810c4db45007df99b66a945a680`, SHA-256
`b3f098ad5cc63b8552bfbd98996e51678e69faf7c743b73aae2a01379c2df3b3`.
The wheel and matching source archive came from
[publication run 37088715347](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/37088715347).
All 119 included Git files matched the release tree, including all 34 adapter
files. Each client used a fresh profile and mailbox, the wheel executable, and
adapters from that source archive.

Codex CLI 0.160.0 with GPT-6.1 Sol high and Claude Code 2.1.288 with
Opus 5.5 high completed both initiating directions. Four native notices led to
full show results before separate ACKs, two correlated replies, and two empty
final native inboxes. All four messages were submitted and ACKed. Codex retained
workspace-write and normal approval review, with individually reviewed host
execution for the exact htalk commands. Claude retained manual per-command
approvals and managed restrictions. Recorded owned processes closed and protected
configuration hashes were unchanged. ACK cleanup was `absent` for Codex and
`unsupported` for Claude; neither proves notification removal.

Earlier core attempts exposed fixture-controller errors, a completed recipient
discovery rejection, and a missed manual approval deadline after confirmed native
delivery and show. They remain separate records without replay. A separate
no-model probe found that an owned host PID was invisible in the default
executor's different PID namespace. The earlier native command did not record
its namespace, so this is a plausible explanation rather than a proven cause of
that rejection. The passing case verified host visibility before sending and
used ordinary approvals for its exact commands.

All three published PyPI files match the checked workflow artifacts. A clean installation outside the checkout passed version, help and dependency checks, and its binary hash matches the one used for the native checks.

The following installed clients each received a native notice, executed show
before a separate request ACK, and produced one correlated reply. The controller
read and ACKed the reply. Each passing case ended with two ACKed rows and empty
inboxes. These checks used local canned provider responses, so they establish
native transport and tool execution rather than model reasoning.

| Client | Version | Permission mode | Native ACK notification cleanup |
| --- | --- | --- | --- |
| OpenCode | 1.18.32 | Other tools denied; only the bound htalk MCP tool allowed | `unsupported` |
| Pi | 0.87.1 | Normal bash tool | `skipped/pull_only` |
| OMP | 18.3.0 | Always-ask; three one-time tool approvals | `skipped/pull_only` |
| OpenClaw | 2026.9.6, revision `eb377ac` | Only htalk enabled; recurring heartbeat disabled | `skipped/pull_only` |
| Cline | 3.0.65, Core 0.0.86 | Automatic approval disabled; three exact one-time approvals | `skipped/pull_only` |
| Kilo | 7.7.9 | Other tools denied; three exact one-time approvals | `unsupported` |
| Goose | 1.52.0 | Approve mode; three manual `allow_once` approvals | `skipped/pull_only` |
| Hermes | 0.21.5, revision `421f9592` | Native tool loop with the adapter's explicit mailbox command set | `skipped/pull_only` |
| GitHub Copilot CLI | 1.0.88 | Isolated folder trust and one-time wait/tool approvals | `skipped/pull_only` |
| Gemini CLI | 0.61.0 | Isolated folder trust and three `Allow once` tool approvals | `skipped/pull_only` |
| Letta Code | 0.33.0 | Strict mode and shipped always-ask htalk tool; mailbox-only approval callback | `skipped/pull_only` |
| OpenHands | CLI 1.16.0, SDK 1.21.0 | Three separately inspected native `Yes` confirmations | `skipped/pull_only` |

OpenCode used a headless server session. Kilo used a server with an attached
TUI. Goose used a separate managed ACP session. The other watcher and managed
routes delivered notices through their native adapters while the core mailbox
reported pull delivery. Their cleanup status does not claim core push or notice
removal. The controller's ACK cleanup was `skipped/pull_only` in these cases.

The recorded owned processes and listeners closed. Hermes and Gemini completed
the exchange but did not exit normally within the fixture's ten-second bound;
the fixture terminated their owned processes. Their aggregate fixture result
remains false. Hermes also made an extra local provider request rejected by a
guard; its cause remains unresolved. OpenClaw downloaded its remote model
catalogue during startup, although inference used the local provider.

Two earlier Cline fixtures stopped before mail or provider calls. Two earlier
OpenHands fixtures failed on event parsing and confirmation timing; their
requests remained unACKed and had no replies. Later checks used fresh mailboxes
and new requests. These earlier attempts remain separate observations.

Antigravity's ordinary CLI was present, but a compatible managed SDK interpreter
was not found in the checked environments. Agent Zero's framework/runtime was
also unavailable at the checked locations. Neither received a new native check;
this does not establish that no other installation exists. Their older dated
observations remain below.

These checks do not establish native crash durability, recovery after physical
network loss, exactly-once external work, or automatic failover. Mailbox schema
remains 3. See the [0.11.0 upgrade requirements](releases/0.11.0.md) before
restarting managed receivers.

Checked on 2026-10-01 UTC with the htalk `0.10.0` publication wheel,
commit `0498932f887865737a6df887773c76c14954dd51`, SHA-256
`3bdd20ad5c0af3704b55948efbf097b3bd03832c187229cc16c331405a22038c`.
Codex CLI 0.159.3 and Claude Code 2.1.285 completed both initiating directions
with GPT-6 Luna. The [publication and catalogue checks](checks/2026-10-01-catalog.md)
record notices, show before separate ACK, reply correlation, permissions,
client versions, cleanup limits and the unsuccessful bare-mode preflight.
The [OpenCode check](opencode.md) used a local canned provider. Both devices
installed the matching PyPI binary while preserving the existing idle worker
and receiver ledger. Schema remains 3. Other adapters retain their earlier
dated observations below and were not rerun for this feature release.

Checked on 2026-09-29 UTC with the htalk `0.9.6` x86-64 publication wheel,
commit `5c8955caf1a3b419d092ff7dc0c2cb9fdb0ce370`, SHA-256
`4abcfe1786432111d9cd17ab6895ca5922cc31d3fb6087217a25dccd907fdbc1`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36506294790)
passed both Linux architecture builds and installed-wheel checks. All 100
included Git files matched the release tree. All three PyPI files and a fresh
installation matched the checked artifacts.

Codex CLI 0.158.0 with GPT-6 Luna medium and Claude Code 2.1.284 with Opus 5.5 high
completed both initiating directions: four native notices, show before each
separate ACK, two correlated replies, no repeated sends or replies, and two
empty final inboxes. Codex used workspace-write and normal approval review
with approved host commands; Claude used manual permissions with the exact
fixture executable allowed. Both terminal sessions closed and their peers
retired. ACK cleanup was `absent` for Codex and `unsupported` for Claude;
these observations do not establish notification removal.

The same wheel rejected an absent explicit receiver socket before invoking
the connector or creating receiver state. Its diagnostic included the path,
the underlying error and the command to inspect saved state. Both devices
then installed the matching PyPI binary; the laptop's existing worker, five
receipts and receiver ledger were preserved without another model turn.
Mailbox schema remains 3; updating from 0.9.5 needs no migration.

The [OpenCode check](opencode.md) used a local canned provider. Post-publication
[WebUI and TUI checks](checks/2026-09-29-native-session-checks.md) used real Luna
with the same wheel and unchanged adapter sources. The separate
[LAN interruption checks](checks/2026-09-29-lan-interruption.md) cover physical
Wi-Fi loss, sleep and reboot on 0.9.5. Starting or supervising an agent is
outside htalk's scope. Other harnesses retain their earlier observations.

Checked on 2026-09-28 UTC with the htalk `0.9.5` x86-64 publication wheel,
commit `709e5ea8f855ca19296203f65a01bc94fb2b066a`, SHA-256
`53f179a5ab74686d9acfe25e7ed0dc8069e5753f362498667db8492f0b9241b6`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36482432457)
passed both Linux architecture builds and installed-wheel checks. All 98 included
Git files matched the release tree. The three PyPI files and a fresh installation
matched the checked bytes.

Codex CLI 0.158.0 with GPT-6 Luna medium and Claude Code 2.1.284 with Opus 5.5 max
completed both initiating directions: four native notices, show before each
separate ACK, two correlated replies, no repeated sends or replies, and two
empty final inboxes. Codex used workspace-write and normal approval review
with approved host commands; Claude used manual permissions with the exact
fixture executable allowed. Both sessions closed, all six recorded processes
were absent, and their peers retired. ACK cleanup was `absent` for Codex and
`unsupported` for Claude; these observations do not establish notification removal.

The [receiver recovery check](checks/2026-09-28-receiver-rebind.md) separates
source-candidate daemon recovery from the publication wheel's laptop checks.
Both devices installed the matching PyPI binary while preserving the idle
worker and its receipts. The [OpenCode check](opencode.md) used a local canned
provider. Other harnesses retain their earlier observations. Physical Wi-Fi
loss, sleep, reboot and automatic worker startup remain untested. Schema is 3.

Checked on 2026-09-28 UTC with the htalk `0.9.4` x86-64 publication wheel,
commit `cab29eabc7d2671bdb9dfacfddb4ad6303459a05`, SHA-256
`89494cd0b9b7e6f18cdb3fd13fff7780df0e90e89347979bde8bb0a994844649`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36429231142) passed both Linux architecture builds and
installed-wheel checks. All 97 included Git files matched the release tree.
The three PyPI files and a fresh installation matched the checked bytes.

Codex CLI 0.158.0 with GPT-6 Luna medium and Claude Code 2.1.283 with Opus 5.5 max
completed both initiating directions in separate Linux terminals. Four native
notices led to show before separate ACK, two correlated replies, no repeated
sends or replies, and two empty final inboxes. Codex used workspace-write and
normal approval review with approved host commands. Claude used manual
permissions with the exact fixture executable allowed. Both sessions were
closed, their eight recorded processes were absent, and their peers retired.
ACK cleanup was `absent` for Codex and `unsupported` for Claude; these statuses
do not establish notification removal.

The [daemon and laptop check](checks/2026-09-28-codex-daemon-laptop.md) distinguishes
a source-candidate computation from a later read-only task on the exact wheel.
The latter reached the same remote Luna through an explicitly bound Codex
server socket and preserved prior artifacts and receipts. Both devices then
installed the matching PyPI binary; replacing the temporary receiver binary
kept the same idle model session without another model turn.

The [OpenCode check](opencode.md) used a local canned provider. Other harnesses
retain their earlier observations. Physical Wi-Fi loss, sleep and reboot remain
untested; automatic worker startup is still open. Mailbox schema remains 3.

Checked on 2026-09-28 UTC with the htalk `0.9.3` x86-64 publication wheel,
commit `e98701dbb2c01194254341603f5f8290afddceee`, SHA-256
`49cf45b55ec31d8bc17852827cbf74d1098e731e8b94f5f977c215cd098829bc`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36360798674) passed both Linux architecture builds and
installed-wheel checks. All 96 included Git files matched the release
tree. Published PyPI files and a fresh installation matched the checked bytes.

Codex CLI 0.157.1 with GPT-6 Luna medium and Claude Code 2.1.283 with Opus 5.5 max
completed both initiating directions in separate Linux terminals. Four native
notices led to show before separate ACK, two correlated replies, no repeated
sends or replies, and two empty final inboxes. Codex used workspace-write and
normal approval review with approved host commands. Claude used manual
permissions with the exact fixture executable allowed. Both fixture sessions
were closed and their peers retired. ACK cleanup was `absent` for Codex and
`unsupported/client_has_no_notification_removal` for Claude; these observations
do not establish notification removal.

The same wheel passed the [current-mail and store check](checks/2026-09-28-receiver-current-mail.md).
Two completed messages were skipped; one ACKed unanswered request produced one
native queue entry. Changing a private alias config immediately before the
real Codex CLI started did not redirect that write. The entry was read back
and deleted without starting another model turn. Receiver ownership also
rejected a second state directory for the same saved session.

The [OpenCode check](opencode.md) used a local canned provider. Other harnesses
retain their earlier observations. Physical Wi-Fi loss and sleep remain
untested. Schema remains 3.

Checked on 2026-09-27 UTC with the htalk `0.9.2` x86-64 publication wheel,
commit `4b8c27cb848a75f33cf4edd0ade15fa320c0fccc`, SHA-256
`bf3e6667f383fd7f82f942dccd5893d21f3627d93ee2e7a6faad5c092cfba75c`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36314173772)
passed both Linux architecture builds and installed-wheel checks. All 96
included Git files matched the release tree, including every adapter source.

Codex CLI 0.157.1 with GPT-6 Luna medium and Claude Code 2.1.283 with Opus 5.5 max
completed both initiating directions in separate Linux terminals. Four native
notices led to show before separate ACK, two correlated replies, no repeated
sends or replies, and two empty final inboxes. Codex used workspace-write with
normal approval review and approved host commands. Claude used manual
permissions with the exact fixture executable allowed. Both owned sessions
were closed, their recorded processes were absent, and their fixture peers
were retired. ACK cleanup was `absent` for Codex and
`unsupported/client_has_no_notification_removal` for Claude. These results do
not establish notification removal.

The same wheel also rejected a second `receive` process using a different
state directory for the actual saved Codex session. The first receiver stopped
normally and restarted with its original state. This used a ready-only watch
connector: no native notice or model turn was requested by that ownership
check. Alternate Codex homes and cancellation during a native write were
covered by the [receiver contract check](checks/2026-09-27-receiver-ownership.md).
Stop receivers before changing the selected Codex SQLite store; live relocation
is outside the ownership guard checked here.

The separate [OpenCode check](opencode.md) used a local canned provider with
this publication wheel. Other receivers and LAN recovery retain their earlier
observations below and were not rerun for this patch. Mailbox schema remains 3.

Checked on 2026-09-27 UTC with the htalk `0.9.1` x86-64 publication wheel,
commit `ab5a9e897cc176c67b480a3ba98baa0c8b0588d2`, SHA-256
`8ba0e59a798448d69a44df83537c64fc59cdd546ffc4fe5c9e4959ca891c5cf1`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36308690259)
passed both Linux architectures. All 95 included Git files matched the release
tree, including the socket helper and adapter sources.

Codex CLI 0.157.1 with GPT-6 Luna medium and Claude Code 2.1.283 with Opus 5.5 max
completed both initiating directions in separate ordinary Linux terminals.
Four native notices led to show before separate ACK, two correlated replies,
no repeated sends or replies, and two empty final inboxes. Codex used normal
workspace-write and approval review with approved host commands; Claude used
manual permissions with the exact fixture executable allowed. Both owned
sessions were closed and their fixture peers retired.

Preflight checks failed inside the sandbox and when a host diagnostic could
not find `claude` on its PATH. Both happened before any send. Corrected host
checks confirmed the exact running sessions before the first request.
ACK cleanup was `absent` for Codex and
`unsupported/client_has_no_notification_removal` for Claude; this does not
establish notification removal. The separate [OpenCode check](opencode.md)
used a local canned provider and the same publication wheel.

The [SSH lifecycle check](checks/2026-09-27-ssh-lifecycle.md) used installed
htalk 0.9.0 before this patch and independently checked automatic tunnel recovery
and explicit same-session resume. It is not another LAN trial of the 0.9.1 wheel.
Other receivers retain their earlier observations below and were not rerun
for this patch. Mailbox schema remains 3.

Checked on 2026-09-27 UTC with the htalk `0.9.0` x86-64 publication wheel,
commit `5e20371fde81e85dec06727f7be84f162aefb14f`, SHA-256
`efb3d73970254d3be155f96e616b75603847dd34c63b62997a0928b4b6e5bf92`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36292771193)
passed both Linux architecture checks. All 91 included Git files matched the
release tree, including the adapters and remote receiver.

Codex CLI 0.157.1 with GPT-6 Luna medium and Claude Code 2.1.283 with Opus 5.5 max
completed both initiating directions in separate ordinary Linux terminals.
Four native notices led to separate show and ACK commands, two correlated
replies, and two empty final inboxes. No send or reply was repeated. Codex used
workspace-write and normal approval review, with approved host access for its
native notification commands. Claude used manual permissions with the exact
fixture executable allowed. Both owned sessions were closed, their recorded
processes were absent, and their fixture peers were retired.

ACK cleanup was `absent` for Codex and
`unsupported/client_has_no_notification_removal` for Claude. These statuses do
not establish notification removal. The separate [OpenCode check](opencode.md)
used a local canned provider with the real MCP tool path.

The new remote path also completed a [Linux LAN check](checks/2026-09-27-ssh-reconnect.md)
with a source-built binary. The publication wheels passed the remote MCP and
receiver contracts. The LAN result is not a second live check of the publication
wheel. Other receivers retain their earlier observations below and were not
rerun for 0.9.0.

Checked on 2026-09-26 UTC with the htalk `0.8.1` x86-64 publication wheel,
commit `ca2e237a054d79b051a34962cff05b5d0178477f`, SHA-256
`b5aed6d76e73e479c4a257df1e4a5924f815295b219fe4516e8268a56b77a399`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36271901091)
passed both Linux architecture checks. Every included source and adapter matched
the release commit. All three published PyPI files matched the checked hashes;
a fresh PyPI installation produced the same executable.

Codex CLI 0.157.1 (GPT-6 Sol medium) and Claude Code 2.1.283 (Opus 5.5 max)
completed both initiating directions on that wheel in separate Linux terminals:
four native notices, show before each separate ACK, two correlated replies,
no repeated sends and two empty final inboxes. Codex kept workspace-write and
normal automatic approval review with approved host commands; Claude kept manual
permissions with the exact fixture executable allowed. Codex's initial network
failure recovered automatically; the single queued start marker was not resent.
Owned sessions were closed and their peers retired. Native ACK cleanup was
`absent` for Codex and `unsupported/client_has_no_notification_removal` for Claude.
These results do not establish notification removal. The independent
[OpenCode server-session check](opencode.md) also passed.

The Agent Zero pause correction has one regression that fails on the old receiver
and passes on the new one. A focused fixture also ran the pinned native scheduler,
extension decorator and acceptance/task-end hooks. It retained pending mail after
a late pause and accepted one wake after resuming. A full Agent Zero WebUI runtime
was not installed for this check. Native check/start remains non-atomic; see the
[integration limits and plugin update steps](../integrations/README.md#agent-zero).
Other receivers' 0.8.0 observations below were not rerun for this patch.

Checked on 2026-09-26 with the htalk `0.8.0` x86-64 publication wheel from
commit `8a07ffc22b38a006404b64eb7c7b0e0a62689c40`, SHA-256
`fce59255b26a07dad0a521adcd5cfcb90fc2100313d79a9f04cd70ffd615996d`.
The wheel was installed in a fresh environment before upload approval. Receiver
files came from the same [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36190781359)'s
source archive; every included source and adapter matched the commit.

Codex CLI 0.156.0 (GPT-6 Sol medium) and Claude Code 2.1.282 (Opus 5.5 max)
completed both initiating directions in separate Linux terminals. Four native
notices led to show, separate ACK and two correlated replies. No message was
resent. Both final inboxes were empty and the owned sessions were closed.
Codex retained workspace-write and normal automatic approval review, using
approved host execution for native notification access. Claude used manual
permissions with the exact fixture executable allowed. An initial sandbox
peer check failed before any send; a distinct authorized continuation completed
the host check before using the still-unused request ID. An earlier tmux launch
failed before creating a session or mailbox and was preserved separately.

The new managed receivers also completed real `openai/gpt-6-luna` exchanges
through OpenAI Flex on this package:

| Receiver | Native interface | Calls | Verified result |
| --- | --- | --- | --- |
| Goose 1.52.0 | ACP | 10 | Same-session restart, delegated sum and original marker |
| Letta Code 0.33.0 | Local headless stream | 9 | Same-conversation restart, delegated product and original marker |
| Antigravity SDK 0.1.18 | Local SDK conversation | 10 | Same-ID resume, delegated product and original marker |

Each case used one real model agent and a synthetic helper. All four linked
messages were ACKed, both native turns completed, the first resumed model
request retained the original context, and the observed two-second idle period
made no inference calls. Total reported provider cost was $0.0025156725. Letta
also refused missing/changed owner tasks before starting a native process and
ignored a peer's request to contact an out-of-scope recipient in this case.
Task scope remains model guidance, not a per-command policy. Antigravity used
an external gateway translating `tool_choice: "none"` to `auto`; that gateway
is not shipped. These checks cover dedicated managed sessions, not attachment
to existing terminals or IDE windows. Owned fixture processes were stopped.

Pi, Oh My Pi, Hermes classic CLI, OpenClaw Gateway, Cline, Kilo, OpenHands,
Copilot CLI and Gemini CLI each completed native tool exchanges with
local canned responses on the same wheel and source archive. All request and
reply links and ACKs were independently checked in their synthetic mailboxes.
These checks establish native tool execution and delivery, not model reasoning.
The [OpenCode server-session check](opencode.md) was independent.

| Installed client | Fixture permissions |
| --- | --- |
| Pi 0.87.1 | `--no-approve`, Bash tool |
| Oh My Pi 18.3.0 | `--auto-approve`, read/write plus MCP |
| Hermes 0.21.5 | `--toolsets htalk`; exclusive tool restriction unproven |
| OpenClaw 2026.9.6 | Additive `tools.alsoAllow: [htalk]`; other tools remained exposed |
| Cline 3.0.65 | `--auto-approve true`, interactive act mode |
| Kilo 7.7.9 | Deny by default, `htalk_*` allowed, MCP ready before mail |
| OpenHands 1.16.0 | `--always-approve` |
| Copilot CLI 1.0.88 | Fixture MCP allowed; exact scratch-folder and shell approvals |
| Gemini CLI 0.61.0 | Trusted scratch workspace, no OS sandbox; deny-all tool policy with a higher-priority htalk MCP allow rule |

The managed cases used `--allow-mail`. Goose disabled its default tools and
mediated the bound htalk calls. Letta's mod restricted access to its configured
htalk server. Antigravity used the pinned SDK configuration and a default-deny
tool policy allowing htalk. These configurations do not establish OS isolation.

Some fixtures had narrower results than a clean overall pass. Pi's final audit
used an unsupported inbox flag after completing its exchange. Hermes completed
the exchange, but an auxiliary title request exceeded the canned provider's
limits and required forced CLI cleanup. OpenClaw completed show/ACK/reply;
the fixture's expected transcript table was absent, so native end-of-turn
status was not established. Kilo's first canned script ended after request A
even though B was already in the active turn. That case remains incomplete.
A fresh script handled both notices once in the same session: six completed
tools, four ACKed rows and native final output. Its status API returned no
session entry, so the fixture's explicit idle assertion remained unproven.
None of these failed cases was replayed. Copilot and Gemini completed their
mail exchanges; their natural CLI exit was not separately observed, and the
fixture stopped their owned tmux servers. Gemini preserved its unsent draft.

Retained native ACK outputs reported `absent` for Codex and
`unsupported/client_has_no_notification_removal` for Claude, OpenCode and Kilo.
OMP, Hermes, Cline, OpenHands, Copilot, Gemini and the three managed receivers
reported `skipped/pull_only`. Pi and OpenClaw's native cleanup outputs were not
retained; their stored ACKs do not establish a cleanup result. Controller ACK
cleanup was captured as `skipped/pull_only` for Copilot, Gemini and all three
managed cases; the other controller cleanup receipts were not retained.
None of these statuses establishes receiver-created notice removal or native
process shutdown. Those are separate from stored ACKs and correlated replies.

Agent Zero's temporary runtime was absent and was not reinstalled for this
wheel check. Cursor's prior schema-only observation does not establish a
receiver; its exact Luna provider route remains unverified. Grok Bot and Manus
were not checked because account access was unavailable. Earlier observations
below retain their own versions, artifacts and limitations.

Checked on 2026-09-25 with the htalk `0.7.0` x86-64 publication wheel from
commit `c18c87b3c0b654f32e1dca127fdb1d8c2aba3a48`, SHA-256
`f97968e1bb8dbca6b8e49dee39889f1e95fb74cf2ec3d6d6de9ebcf993baf232`.
The wheel was installed in a fresh environment, and receiver files came from
that publication run's source archive. All source and adapter files matched the
commit. [Publication build](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36150459090).

Ordinary Codex CLI 0.156.0 with GPT-6 Sol medium and Claude Code 2.1.282 with
Opus 5.5 max completed both initiating directions in separate Linux terminals.
Four native notices were consumed, all four messages were shown and acknowledged,
and both correlated replies arrived without repeated sends. Both final inboxes
were empty; the sessions closed and their fixture peers were retired. Codex
retained its sandbox and automatic approval review, with approved host execution
for notification access. Claude used manual permissions with the fixture
executable allowed. ACK cleanup was `unavailable/codex_rpc_closed` for Codex and
`unsupported/client_has_no_notification_removal` for Claude. These results do not
establish notification removal.

The same wheel completed native exchanges with local canned model responses in
Pi 0.87.1, Oh My Pi 18.3.0, Hermes classic CLI 0.21.5, OpenClaw 2026.9.6, Cline
3.0.65/Core 0.0.86, Kilo 7.7.9, OpenHands CLI 1.16.0/SDK 1.21.0, Copilot CLI
1.0.88, Gemini CLI 0.61.0, and a separately controlled Goose 1.52.0 ACP session.
The [OpenCode check](opencode.md) used its native server session. These checks
verify client tool execution and mailbox behavior, not model reasoning.

The fixtures used only synthetic mail. Pi used `--no-approve`, OMP
`--auto-approve`, Cline `--auto-approve true`, and OpenHands `--always-approve`.
Copilot used exact one-time approvals for its waiter and allowed the fixture MCP
tool. Goose disabled default tools and mediated each htalk call. Kilo initially
denied Bash; its MCP phase denied all tools except `htalk_*`. Gemini trusted its
scratch workspace and used a deny-all policy with a higher-priority allow rule
for the htalk MCP tool. Hermes selected `--toolsets htalk` with tool search
disabled; OpenClaw used the additive `tools.alsoAllow: ["htalk"]` with tool search
disabled. Hermes and OpenClaw ran htalk without an interactive approval prompt,
but these fixtures supplied no explicit approval or sandbox policy and did not
establish that other tools were prohibited. Approval gates were not exercised
by those two fixtures.

Hermes completed its agent turn, but its auxiliary title request hit the test
provider's limit and the fixture had to terminate the CLI. OpenClaw completed
its original saved request after a provider-fixture size rejection, without
resending it. After enabling MCP, Kilo's fixture reintroduced the two saved
notices into the same session to complete show/ACK/reply. It created no new
mailbox requests; completion required this second phase. Copilot handled the
request sent during its busy interval after the interval; admission before the
first turn ended was not observed. Gemini kept an unsent draft. Owned fixture processes were stopped.

The clients' own ACK outputs recorded these `notification_cleanup` results:

| Client | Status / detail |
| --- | --- |
| Codex | `unavailable/codex_rpc_closed` |
| Claude Code | `unsupported/client_has_no_notification_removal` |
| OpenCode | `unsupported/client_has_no_notification_removal` |
| Pi | `skipped/pull_only` in a separate follow-up; first receipt not captured |
| Oh My Pi | `skipped/pull_only` |
| Hermes classic CLI | `skipped/pull_only` |
| OpenClaw | `skipped/pull_only` |
| Cline | `skipped/pull_only` |
| Kilo | `unsupported/client_has_no_notification_removal` |
| OpenHands | `skipped/pull_only` |
| Copilot CLI | `skipped/pull_only` |
| Gemini CLI | `skipped/pull_only` |
| Goose ACP | `skipped/pull_only` |

`skipped/pull_only` describes the core's cleanup decision; it does not establish
removal of a notice created by a receiver. Pi's first stored ACK and empty inbox
were verified, but its cleanup receipt was not retained. One fresh canned
exchange on the same wheel captured the native tool result shown above; it does
not fill the missing receipt from the first exchange. For the canned exchanges,
the controller's reply ACK cleanup was captured for Copilot and Gemini as
`skipped/pull_only`; the other controller ACK rows were verified in storage,
but their exact cleanup outputs were not retained. ACK storage, notice removal
and owned-process cleanup are separate observations.

Letta Code 0.33.0 passed native MCP discovery and a CLI tool call; Cursor Agent
2026.09.23-86fc751 passed schema discovery only. Letta's show-only request was
intentionally left unacknowledged, so cleanup was not attempted. Cursor had no
message to acknowledge. Agent Zero and Antigravity's
previous temporary runtimes were no longer installed for this wheel check.
Their earlier source observations below remain separate from release-wheel
verification. The managed Goose experiment is tracked in
[issue #25](https://github.com/jointsome0-lgtm/harness-talk/issues/25) and is not
shipped as a receiver.

The [Pi, Hermes, OpenClaw and Agent Zero source integrations](../integrations/README.md) use a shared
`htalk watch` receiver. On 2026-09-25, Pi 0.87.1 in RPC mode and Hermes 0.21.5 in
classic CLI mode completed a Linux exchange with GPT-6 Luna through OpenRouter
Flex. An incoming notice woke Pi; Pi asked Hermes for a calculation, Hermes
answered through its `htalk` tool, and Pi returned the independently checked
result to the original sender. All four message links and acknowledgments were
verified in an isolated mailbox. Restarting the receivers recovered the saved
requests without sending new copies. Those checks used the pre-release source build. Hermes gateway and modern TUI
remain unsupported.

The reverse Hermes → Pi exchange also completed after resuming Hermes's saved
session. The first attempt exhausted its verification budget after reading and
acknowledging Pi's answer. Recovery exposed the htalk tool directly, prohibited
new sends, and answered the original request without repeating the delegation.
All four saved messages and ACKs were checked. This demonstrates recovered
completion, not an uninterrupted first-attempt pass.

OpenClaw 2026.9.6 then completed controller → OpenClaw → Pi → OpenClaw → controller
using its built-in runtime, the same mailbox, and Luna/Flex. The Gateway's
targeted notification wake started the configured main session with periodic
heartbeat and cron disabled. All four messages, reply links, ACKs and the
arithmetic result were checked. There were no model calls before the first
message. After the saved exchange completed, the fixture's ten-request cap
blocked further OpenClaw continuation. A separate model-free check verified
plugin loading, a 25-message backlog, session-scoped tool access and watcher
cleanup.

A fresh Pi → OpenClaw → Pi exchange with direct tool schemas then completed
within both request caps. Four correctly linked messages were acknowledged;
Pi independently checked OpenClaw's answer. OpenClaw recorded native session
status `done` and trajectory outcome `success`, with no guard rejection. The
temporary runner incorrectly expected `completed`, so it reported a timeout;
the saved mailbox and native trajectory established the successful result
without rerunning the exchange. These checks do not cover other OpenClaw
runtimes or operating systems.

Agent Zero revision `e3051fb584b1a36be2b0a0c90606f1c2c2d356ec` was checked
on Python 3.12 and Linux with its actual context registry, plugin loader,
extension hooks and tool class. The model-free checks exercised a paused/busy
backlog, user-queue priority, context-scoped tool access, CLI inbox/show/ack/reply,
context replacement, plugin reload/deletion and watcher cleanup. A separate
ordinary framework turn used a local canned model response: an incoming notice
woke the agent, exposed the htalk tool prompt/schema and ended through the native
response tool. Idle receivers made no model calls. A local wire capture checked
Luna/Flex settings for chat, utility and vision; it made no external API request.

Agent Zero then completed controller → Agent Zero → Pi → Agent Zero → controller
on Luna/Flex. All four messages and ACKs were checked independently; Pi returned
272 for 16×17. The original model turn reached its deadline after saving the
answer. A separate recovery of the saved chat ended the native turn with one
remaining model call, without sending more messages or invoking Pi again.
This establishes recovered completion, not an uninterrupted first-attempt pass.
These checks do not establish WebUI/container installation, other operating
systems or full startup with default plugins and embeddings. The pinned Agent Zero API also has a
narrow concurrent pause race, documented in [setup](../integrations/README.md#agent-zero).

The [common MCP interface](../integrations/mcp.md#client-setup-and-verification)
records additional native client checks separately from automatic notification
receivers and completed model exchanges.

Checked on 2026-09-23 with the htalk `0.6.1` x86-64 publication wheel from commit `6b6b9efef90455ecff640049f38d4e14f15ace37`, SHA-256 `89befb955c10b8b49796bfcdae3f3b2e03fabc98dd84d192dd2858aeaba84e75`. Its published PyPI file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-6-astra`, low) ran in separate Linux tmux terminals. Claude retained manual permissions with the fixture executable allowed; Codex retained `workspace-write`, on-request escalation and automatic approval review, using approved host execution for each htalk command.

The isolated mailbox was created with htalk `0.5.1`, with two native peer registrations and no messages. A normal `peer list --all` using 0.6.1 upgraded schema 2 to 3. The one private backup matched the legacy SQLite dump, and the original peer fields were preserved. A pull participant was then registered with 0.6.1.

Each native client initiated a request and consumed the other's correlated reply. Codex → pull and pull → Claude also completed. All eight messages were shown and acknowledged separately, with each request acknowledged before its reply. Six native notices were submitted and consumed once; two pull messages made no notification attempt. All three final inboxes were empty, the clients exited and the peers were retired. No message or notification was repeated. Codex cleanup was `absent`, Claude cleanup `unsupported`, and pull cleanup `skipped/pull_only`.

A separate [OpenCode 1.18.31 check](opencode.md) used the same wheel and completed both initiating directions with a pull participant. These are Linux observations; direct OpenCode ↔ Codex/Claude and cross-device exchanges remain unchecked.

Checked on 2026-09-23 with the htalk `0.6.0` x86-64 publication wheel from commit `9963bb4dee3eeb1a6b63ea64978b34dd1977e109`, SHA-256 `c70f8990a8d66cecad7f96747f6f6dacaaa9721f5532ab8725ef6c42579ad23b`. The wheel was installed in a fresh environment before PyPI approval; the published file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-6-sol`, medium) ran in separate Linux tmux terminals with an isolated schema-3 mailbox. Claude used manual permissions with the test executable allowed. Codex retained `workspace-write`, on-request escalation and automatic approval review; each htalk command used approved host execution.

Codex and Claude each initiated a request and consumed the other's correlated reply. A Codex request to a pull participant and a pull request to Claude also completed with correlated replies. All eight messages were shown and acknowledged in separate commands, with acknowledgment preceding each reply. Six native notices were submitted and consumed once; the two pull messages made no notification attempt. No send, reply or notice was repeated, no answer was polled through `wait`, and all three final inboxes were empty. The native sessions were closed and their test peers retired. Codex cleanup was `absent`, Claude cleanup was `unsupported`, and pull cleanup was `skipped/pull_only`; acknowledgment does not imply notification deletion.

A separate [OpenCode 1.18.31 check](opencode.md) used the same publication wheel and completed both initiating directions with a pull participant. These observations cover all three native clients with the common mailbox on Linux; direct OpenCode ↔ Codex/Claude and cross-device exchanges were not tested.

Checked on 2026-09-22 UTC with the htalk 0.5.1 x86-64 release wheel from commit `ff28ef3c55b952d7900ee076b37a8a0bf4086ba5`, SHA-256 `be86ab5aeed823c648912949b397776ee71b8206384e84c844cbab6b234ce502`. The publication workflow's wheel was installed in a fresh environment before PyPI approval; the published PyPI file has the same hash. Ordinary Claude Code `2.1.280` (Opus 5.5, max) and Codex CLI `0.156.0` (`gpt-5.6-luna`, low) TUI sessions ran in separate tmux terminals on Linux with an isolated synthetic database. Claude used manual permissions with the test executable allowed. Codex retained `workspace-write`, on-request escalation and automatic approval review; every htalk command used approved host execution.

One request and its correlated reply completed in each direction. All four notifications were submitted, consumed through native notices, read and acknowledged. Neither recipient needed controller assistance to retrieve an answer. The initial Codex command used a nonexistent `request` subcommand and was rejected before storage; after verifying that no message existed, the controller supplied the correct `send` syntax. In the reverse exchange Codex read the request, replied, then acknowledged it, departing from the planned acknowledgment order. This verifies the transports with those setup and ordering deviations; it was not a fully unassisted execution of the test protocol.

Both Codex acknowledgments reported `notification_cleanup: absent`, because their notices had already been consumed. Both Claude acknowledgments reported `unsupported` with `client_has_no_notification_removal`. No notification or cleanup was retried, and no answer was polled through `wait`. OpenCode was not installed for this check; its earlier observation remains in [OpenCode sessions](opencode.md).

Checked on 2026-09-21 with the htalk 0.5.0 x86-64 release wheel from commit `fb6f93ea85372484749cc86ab062d8e511107923`, SHA-256 `46c4db64751ba6b56b5c13d83a8b74e1eb818745712af65fed11beca82448e87`. The wheel was downloaded from the publication workflow and installed in a fresh environment before PyPI approval; the published file has the same hash. Ordinary Claude Code `2.1.278` (Opus 5, medium) and Codex CLI `0.154.0` (`gpt-5.6-luna`, low) TUI sessions ran in separate tmux terminals on Linux, using an isolated synthetic database. Claude used manual permission mode with the test htalk command allowed. Codex used a `workspace-write` sandbox, on-request escalation and automatic approval review.

A Codex-initiated request and its Claude answer were submitted, consumed through native notices, read and acknowledged; the answer's `in_reply_to` matched the request. In the first reverse case, Codex omitted the requested host escalation: its answer was saved, but the Claude notification was `not_submitted` with `recipient_unavailable`. Claude recovered that answer by an explicitly instructed `show` and `ack`, without a resend. In one distinct corrective case, the new request's notice arrived before Codex processed its control instructions. Codex read and acknowledged the request, then stopped at the unexpected body. After an explicit controller continuation, it replied through the approved host scope; Claude consumed the native answer notice, read the correlated answer and acknowledged it. This corrective case verifies the transports with host access, but required controller intervention.

Across those three requests and three replies, all six messages were acknowledged; five notifications were submitted and one was not submitted. Codex's three acknowledgments saved the read mark but reported `notification_cleanup: unavailable` with `codex_rpc_closed` inside its sandbox. Claude's three reported `unsupported` with `client_has_no_notification_removal`. Cleanup was not retried, and no notification was replayed. OpenCode was not installed for this release check; its earlier observation remains in [OpenCode sessions](opencode.md).

Verified on 2026-09-08 with htalk 0.1.1: ordinary Codex `0.153.4` and Claude Code `2.1.263` TUI clients in tmux on Linux consumed native notifications without manual recipient advancement. Both initiating directions received, read and acknowledged the exact correlated answer. These are compatibility observations, not a guarantee for other builds.

In the Claude-initiated exchange, Codex could not discover Claude from its ordinary tool execution scope: the reply was saved, but its notification was `not_submitted`. Claude recovered and acknowledged the answer through `--wait`. In the reverse exchange, Codex used its standard approved host command scope for the read-only `peer check` and a single send. Idle Claude consumed the notification and replied; Codex read and acknowledged the answer, then later consumed its queued notice. Three of the four notifications were submitted; none was replayed.

Checked on 2026-09-17 with the htalk 0.4.0 release wheel built from commit `fae93218bc5fcdb66f8e67ca0d1be2eb4117814e`, with SHA-256 `254b3b7e67a4f6d5e9e46ea57d0817872c34fca755946cfd169749e05f389db2`; PyPI serves the same file. Claude Code was checked before the release was published, and Codex after. Two ordinary Claude Code `2.1.274` TUI sessions ran in tmux on Linux, in manual permission mode with `htalk` commands allowed. One sent a request. The other consumed its notice, then read, acknowledged and answered it. The first consumed the answer's notice, then read and acknowledged the answer. Both notifications were `submitted` with `claude_socket_bytes_written`. The answer's `in_reply_to` matched the request. Both acknowledgments reported `notification_cleanup` as `unsupported` with `client_has_no_notification_removal`. Claude Code showed each notice as a message from another Claude session, followed by its own guidance for peer messages.

An ordinary Codex `0.154.0` TUI then exchanged one request and its answer in each direction with a Claude Code session set up the same way. Codex used `gpt-5.6-luna` at low reasoning effort and the user's permission settings: a `workspace-write` sandbox and on-request escalation with automatic approval review. `--add-dir` made the database directory writable. A Codex TUI started without a prompt had no saved thread, so this one answered one prompt before registration. Each recipient was idle when its notice arrived; it consumed the notice and read the message. All four notifications were `submitted`: `codex_cli_queued:QUEUE_UUID` toward Codex and `claude_socket_bytes_written` toward Claude Code. Both answers' `in_reply_to` matched their requests. As instructed, Codex requested escalation for `peer check`, `send` and `reply`, which reach the Claude Code socket; each was approved without a prompt. Codex ran `show` and `ack` in the sandbox. Both of its acknowledgments saved the read mark and reported `notification_cleanup` as `unavailable` with `codex_rpc_closed`. Repeated outside the sandbox, cleanup reported `absent`, because Codex had already consumed both notices. The Claude Code acknowledgments reported `unsupported`, as above.

## Discovery results

`peer discover` returns `sessions` with exact IDs, workspaces, runtime evidence and connection details. `sources` reports `ok`, `partial` or `unavailable`. An empty result describes only the inspected environment: sandboxes, process namespaces, stopped servers and custom client homes can limit coverage. Addresses are a snapshot; `peer check` and notification preflight verify them again.

| Source | Runtime evidence |
| --- | --- |
| Claude `agents --json` | `running` requires a live native record and matching session, workspace and owned socket. It does not indicate model activity. |
| Codex app-server | `idle`/`active` describes a loaded thread; `systemError` reports a loaded thread with a runtime problem. |
| Codex writer locks | `writer_active` means a native CLI holds its kernel writer lock, not that a model turn or queue consumer is active. |
| OpenCode server | `idle`, `busy` or `retry` is the server's state. An idle session need not have an attached TUI. |
| OpenCode saved metadata | `unknown` only establishes a saved, unarchived session. Supply its running server URL before sending. |

Use `--workspace` or `--harness` to filter. Repeat `--codex-socket` or `--opencode-url` to inspect explicit servers. The defaults and limits for each client follow below.

## Codex

Ordinary local TUI sessions use `codex queue --thread EXACT_UUID --message TEXT`. `htalk` passes no model, sandbox, approval, profile, or configuration overrides and never types into tmux. Both Codex routes reread the saved message state after their identity check and before queueing; an acknowledged message, or an answer the requester's `wait` has already returned, is not queued.

Before invoking that command, `htalk` opens `state_5.sqlite` read-only and selects only the exact session's `id`, `cwd`, `archived`, and `source`. It requires the registered UUID/workspace, an unarchived row and `source=cli`. The metadata directory comes from the user-level `sqlite_home` setting in `$CODEX_HOME/config.toml`, then `CODEX_SQLITE_HOME`, then `$CODEX_HOME`, defaulting to `~/.codex`. Missing metadata, a different workspace, an archived session, or an unexpected schema prevents submission.

This is a version-specific saved-identity check, not a public stable lookup API or proof of an active TUI. `peer check` reports the metadata file and `runtime_status: unknown`. The adapter does not inspect process environments or acquire native writer locks. It does not reproduce Codex's complete layered configuration loader; system/project/profile-only metadata relocations are outside this tested lookup. Use the same local client metadata as the receiving TUI and inspect `peer check` before sending. The CLI itself receives the exact UUID, never a display name.

A successful native receipt must match both `Queued message QUEUE_UUID for thread SESSION_UUID.` and exit code 0. `QUEUE_UUID` is the client queue identifier; the htalk message UUID remains in the durable database and notification text. A timeout, nonzero exit or unfamiliar receipt after the process starts is `submission_unknown`, with no retry. A saved identity cannot guarantee the client will consume the queue; only a reply or explicit acknowledgment establishes later progress.

The native route is grounded in the installed command help, the actual queue receipt, and the official [session queue implementation](https://github.com/openai/codex/blob/main/codex-rs/tui/src/session_queue_commands.rs). That implementation passes UUID targets directly to the queue API and may use an embedded app server internally. `htalk` does not require or manage a persistent app-server process for ordinary TUI sessions.

For previously configured standalone app-server sessions, the explicit `--socket` mode remains available. It verifies Unix socket ownership and exact live UUID/workspace with status `idle` or `active`, then calls `thread/queue/add` once on the same connection. This build requires `experimentalApi: true` and rejects WebSocket compression, so compression is disabled. No explicit-socket failure falls back to the native CLI, which could address a different client environment. This mode does not force queue consumption or start a thread.

The official [App Server documentation](https://learn.chatgpt.com/docs/app-server) describes the Unix WebSocket and metadata-only `thread/read`. The installed CLI's generated schema supplies the experimental queue shape. The [CLI reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli) describes queued input. The Rust executable uses tungstenite for this standalone mode; the pip package has no Python runtime dependencies. Only the inspected client version is claimed as tested.

### Removing acknowledged notifications

`ack` saves the read mark before calling `thread/queue/delete` with the registered thread UUID and the queue ID from that message's confirmed submission receipt. Socket queue IDs are opaque strings and are passed back unchanged. Native CLI receipts require a UUID and are normalized before cleanup. It never deletes the whole queue. A notice already consumed by the client cannot be recalled.

For native CLI addresses, removal repeats the saved-identity check and uses a temporary `codex app-server --stdio` process with the user's normal configuration. It initializes the protocol, deletes the one queue item and closes the process. It never starts or resumes a thread or requests a model turn. Each RPC has one 10-second deadline covering request writing and response reading, with bounded process shutdown. Stdio writes use nonblocking I/O and poll against that deadline. A write failure can leave a partial request and remains uncertain after an attempted submission. No persistent server is installed or managed. Explicit socket addresses use their registered socket and repeat the live identity check, with no native fallback.

The temporary stdio process runs in a private process group. Cleanup retains its unreaped wrapper as the group anchor, uses Linux pidfds for inspected group members, and stops descendants even if the wrapper exits on EOF. It gives EOF one second, then uses a four-second group TERM/KILL observation budget checked between process scans, and reaps the wrapper only after confirmed cleanup. Process scans and scheduling can extend elapsed time. This requires Linux pidfds and readable `/proc` identities. A descendant that leaves the private group is outside this cleanup contract.

Rust callers can use `Rpc::close(&mut self)` to check cleanup. Every close attempt disables later RPC calls, including when cleanup fails. A failed stdio close returns `codex_stdio_cleanup_unconfirmed`, retains the child and group for another cleanup attempt, and omits the EOF grace on retry. Successful close is idempotent. `Drop` makes a bounded cleanup attempt and cannot report its result; after failure it may leave an unreaped wrapper and unconfirmed descendants. Cleanup failure does not change a previously confirmed queue receipt or replay a request. Socket close releases the local transport after bounded close I/O and does not establish remote consumption.

If `ack` races an in-flight submission, the sender tries the same idempotent removal after saving the queue receipt. A crash or unconfirmed submission can leave a queued notice without a known queue ID; htalk does not search for or replay it. See [cleanup results and recovery](reference.md#notifications-and-recovery).

### Discovery

`peer discover` connects to `$CODEX_HOME/app-server-control/app-server-control.sock`, or the explicitly supplied `--codex-socket` paths. It pages through `thread/loaded/list` and reads metadata with `includeTurns: false`. It never resumes or subscribes to a thread. Sessions unloaded during discovery and internal workers that cannot accept direct input are excluded. Each server is bounded to 200 inspected IDs, 20 pagination cursors and a 15-second scan budget, plus any already-running bounded RPC. Truncation or a failed page produces source diagnostics while preserving verified addresses.

On Linux, default discovery also matches exclusive kernel `FLOCK` records in `/proc/locks` to owned UUID files in `$CODEX_HOME/thread-writer-locks/`. Only matching unarchived `source=cli` rows are read from the same SQLite address metadata used before notification. The result is `writer_active`; it does not distinguish an idle client from a running model turn. A lock file without a held kernel lock is ignored. No lock is acquired, released or deleted. Process environments and conversation bodies are not read.

The lock lifecycle follows Codex's [writer ownership implementation](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/writer_lock.rs) and [live recorder lifecycle](https://github.com/openai/codex/blob/main/codex-rs/thread-store/src/local/live_writer.rs). This is a version-specific Linux fallback, verified on ext4. Process namespaces can hide kernel records, and older clients may not use these files. Source results describe the inspected environment, not every client on the host. A socket-backed address takes precedence when both sources find the same thread and workspace.

## Claude Code

The adapter runs `claude agents --json` and selects exactly one live record matching both session UUID and workspace. It reads only that PID's `~/.claude/sessions/PID.json` metadata, verifies the UUID and workspace again, and checks the Unix socket's type and owner. It never reads credentials.

A command run by Claude Code can omit `--as`: htalk then locates the same metadata file through the command's `CLAUDE_PID`, without running `claude`, and verifies process ancestry in `/proc`. See [peer selection](reference.md#database-and-peers) for the checks and their limits. In the 2026-09-17 checks with Claude Code `2.1.274`, every message command run by Claude Code omitted `--as` and reported `actor_source: native_session`. A detached tmux window given one session's `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PID` was refused with `claude_pid_not_an_ancestor`. After `/clear`, that session had a new ID, and htalk selected no peer for it. `claude --resume` with the original ID kept that ID, and the session's peer was selected again.

The frame contains `type: user`, session and message UUIDs, an honest `htalk:PEER` sender, `priority: next`, and a message pointing to the inbox. It does not assert permission classes or impersonate a native Claude peer. A successful socket write proves only that the bytes were written. The adapter does not claim the model saw them.

This transport is an observed local client interface. It is not documented here as a stable public Claude API. Native inbound controls and filesystem permissions remain in force. If discovery fails, the message remains available in the shared inbox with `not_submitted`. `recipient_unavailable` does not establish that Claude is offline: discovery depends on the invoking command's execution scope. Check the peer in the intended send scope first; a known live session may require normal client permission approval for those specific commands.

After discovery and socket connection, the adapter rereads the saved message state before writing the frame. If the recipient acknowledged it during preflight, or its `wait` has already returned this answer, no frame is written. This was checked with ordinary Claude Code `2.1.267` on 2026-09-10 using a controlled preflight delay until after acknowledgment and the current turn's final response. The message remained acknowledged, no notification entered the native queue, and no extra turn appeared during the following minute.

The inspected `2.1.267` ordinary-session socket handler has no message-cancellation action. The SDK's separate `cancel_async_message` protocol does not establish support through this socket. An already accepted Claude notice cannot be withdrawn by htalk; in an earlier native case it arrived after acknowledgment within the current turn. The final database check narrows the race but cannot eliminate delivery that starts after the check and before acknowledgment.

## OpenCode

OpenCode uses an explicit loopback HTTP server and opaque session IDs. The server confirms the session and workspace before one `prompt_async` POST. Read [OpenCode setup and compatibility](opencode.md) for authentication, discovery coverage and how an accepted prompt can start a turn in an idle existing session.

## Adding an adapter

Pull peers already expose the shared CLI without a native adapter: `peer add NAME --harness LABEL --delivery pull`. The label describes the caller and does not install, discover or wake a client.

Keep persistence and conversation rules in `Store`. `Peer` stores an open harness label and delivery mode; `notify` converts native addresses to `NativePeer` and dispatches to the built-in `Harness` registry. Registration normalization belongs in `notify::native_peer`, not storage. New native adapters extend that registry, dispatcher and validation; they do not change the mailbox schema. Unknown adapters leave list, inbox, show and ACK usable, and fail notification with `adapter_unavailable`. A notification function takes the registered recipient and saved message, then returns an `Outcome` with `submission` and `detail`. The shared notification dispatcher provides a read-only callback for the final database check. The store claims the one attempt before calling it. A failure after client submission might have begun must return `submission_unknown`; only a failure known to precede transmission may return `not_submitted`. An interruption leaves the claim uncertain. Report fixed codes with `Failure::Coded`; other failures carry only the corresponding exception class name through `Failure::Class`, preserving the diagnostic contract of 0.4.

An adapter must verify available evidence for the exact session address, never broaden delivery to a name match, and never replay an uncertain attempt. Adding a harness also requires registration validation and focused tests. Shared storage, reply correlation, acknowledgments and waiting remain unchanged.

The [Codex RPC transport](../src/codex/rpc.rs) runs tungstenite over a Unix stream that refreshes the remaining timeout on every underlying read and write. Partial handshake or frame I/O shares the opening, RPC or close deadline of that operation. Its socket mode accepts Unix sockets only; OpenCode has its separate loopback HTTP transport.
