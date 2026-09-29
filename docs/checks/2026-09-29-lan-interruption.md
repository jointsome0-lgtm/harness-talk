# Physical Wi-Fi loss, suspend and reboot

Checked on 2026-09-29 with htalk 0.9.5 on two Linux x86-64 machines. A single
mailbox on the controller was exposed through the existing authenticated SSH
route. The laptop used Codex CLI 0.157.1, app-server daemon 0.158.0 and one
existing GPT-6 Luna medium session with its native permissions.

These checks extend the earlier SSH-process interruption checks. They do not
establish exactly-once external task execution under arbitrary failures.

| Interruption | Delivery and work observed | Recovery |
| --- | --- | --- |
| Owner disabled laptop Wi-Fi for about 136 seconds | One request saved while offline, one later model turn and reply. The 1,024-row, 11-group result matched an independent calculation. | SSH and delivery recovered without restarting the worker or receiver. |
| Owner suspended the laptop for about 121 seconds | The BOOTTIME minus MONOTONIC delta confirmed actual sleep. One request saved during sleep produced one turn and reply. The 512-row, 7-group result matched independently. | Same boot, worker and receiver processes; network and delivery recovered after wake. |
| Owner rebooted and logged in | Boot ID changed. Saved session identity, four receipts and result files survived. Explicit recovery caused no model turn; one subsequent read-only request produced one turn and reply. | SSH recovered automatically. Codex and the worker required explicit recovery; the receiver required rebind and restart. |

Wi-Fi and suspend requests were each saved once. The controller read and
independently verified each result before ACKing its answer. Later observations
showed unchanged result bytes, file identity, modification times, receipt
ledgers and visible task traces. Temporary observers were stopped after the
checks.

## What reboot did and did not prove

The enabled receiver service attempted startup and exited with
`FileNotFoundError`. Its `Restart=no` policy did not schedule another attempt.
The first inspection found its selected Codex socket absent. The short error
alone did not identify the missing path. An isolated check later reproduced
the same diagnostic by passing an absent `--codex-socket`; startup returned
before creating receiver state or attempting notification.

The operator started the installed daemon, resumed the same worker UUID without
a startup prompt, inspected the retained ledger, ran `receive rebind`, and
restarted the receiver. The four old receipts remained and no old task was
replayed. This demonstrates explicit delivery recovery after the agent is made
available. Starting Codex or restoring its session is outside htalk's scope.

The subsequent read-only task asked the worker to hash the existing files. Its
tool computed the correct hashes, but the model duplicated part of one digest
when composing its reply, producing 81 characters instead of 64. The saved
message exactly matched the reply tool's input. The controller rejected the
answer's digest; htalk delivery passed, answer correctness failed. Reading and
ACKing that answer did not accept its result. No second reply or task was sent.

The final observation retained one new turn, five receipts, no pending
submission and unchanged files. This is evidence for the observed route and
versions, not a claim that every harness recovers after every reboot.

## Recovery boundary

Preserve the mailbox IDs and receiver ledger. Diagnose an unavailable target
before changing its binding. A replacement listener at the saved path requires
explicit `receive rebind` after inspecting the existing session and any pending
outcome. An uncertain submission must not be retried automatically.

The receiver owns message delivery and its submission receipts. The harness or
machine setup owns agent startup. Task result validation remains independent
of delivery and ACK.
