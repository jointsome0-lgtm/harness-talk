# Codex receiver recovery after a listener replacement

Checked on 28 September 2026 UTC with htalk 0.9.5. This covers explicit
operator recovery. Physical Wi-Fi loss, sleep and reboot were not exercised.

## Source-candidate recovery

The candidate from reviewed commit `254e88f70ea5aa81279efa19a5d72ce5c597678e`
has the same tree as release commit `709e5ea8f855ca19296203f65a01bc94fb2b066a`.
An ordinary Codex CLI 0.158.0 session using GPT-6 Luna medium completed a
bounded calculation through the receiver and MCP mailbox. The controller
independently checked all 128 input rows, their sum and the SHA-256 result.
The worker used an exclusive execution marker.

After the receiver and idle client stopped, the controller restarted only the
test app-server. `receive status` reported `codex_server_socket_changed`.
Rebind before resume returned `recipient_not_loaded` and preserved state.
The controller explicitly resumed the same UUID and workspace, then ran
`receive rebind`. Only the saved socket fingerprint changed. The original
receiver command restarted with its original receipt.

A separate read-only request then returned the existing result. The marker and
result kept the same bytes, inode and modification time. The session completed
three turns: bootstrap, calculation and readback. Restart and resume added no
model turn. Both receipt IDs remained, pending was null, and all owned test
processes closed after the check.

New threads created through this Codex build's remote TUI were saved as
`vscode`. They completed bootstrap only and were not counted as receiver tests.
The verified case created an ordinary CLI session first and resumed its exact
UUID with saved workspace-write and normal approval-review permissions.
htalk's saved CLI identity check was retained.

## Laptop and publication wheel

On the existing laptop route, the source candidate reported the same idle UUID
and two receipts. Rebind refused while the receiver was running. After stopping
only the receiver, rebind returned `changed: false`; state stayed byte-identical,
and the service restarted. The worker and its daemon were not restarted.

The x86-64 wheel from [publication run 36482432457](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36482432457)
had SHA-256 `53f179a5ab74686d9acfe25e7ed0dc8069e5753f362498667db8492f0b9241b6`.
It separately passed laptop status and active-rebind refusal, preserving state.
The [native release checks](../adapters.md) used this wheel before upload approval.

All three published PyPI files matched the checked artifacts. A fresh install,
both device installations and the running laptop receiver had binary SHA-256
`03ec25ef25d02188ce9f9a2904a4ef98f127fbcaf3c769a563d4a12bc76a990f`.
The laptop worker kept its UUID, process and four completed turns; the two
receipts and state bytes were unchanged. The mailbox was reachable with no
open messages. This installation check sent no new model task.

The first fresh install and first laptop update saw an index without 0.9.5,
despite the completed upload. Both failed before installation. After read-only
index checks showed the version, installation succeeded; upload was not repeated.
The laptop receiver returned to service after each installation attempt.

## Failure boundaries

The extended existing socket test covered listener replacement during the
probe, a different workspace, an unloaded thread, pending-state refusal,
read-only status releasing its lock before RPC, and skipping an old receipted
ID while accepting a new one. The count remained 50 Rust and 61 CLI checks.
Formatting, clippy, Rust 1.88 and CI passed. No dependencies were added.

A receipt records notification submission. It does not establish consumption,
ACK or task execution. Lost queued work and a pending unknown outcome still
need explicit inspection; rebind cannot clear or replay them. Status reports
local state and target availability, not remote-mailbox connectivity.

Rebind preserves one ledger. The receiver's session-wide lock still guards
delivery, but does not merge receipts from independent state directories or
make switching to an older ledger safe. See the [recovery procedure](../../integrations/remote.md#inspect-and-recover-after-a-daemon-restart).
