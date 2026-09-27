# Native Codex reception over SSH, 2026-09-27

The [earlier LAN pilot](2026-09-27-ssh-pilot.md) used pull peers and a separately
started worker turn. This follow-up checked whether incoming htalk notices
could start work in an already-running, idle ordinary Codex TUI on a second
Linux device.

The mailbox and TUI were on the worker laptop. The PC used a dedicated
forced-command SSH key fixing its MCP database and sender. The worker was a
native Codex peer bound to one exact session UUID and workspace; its local
MCP entry accessed the same mailbox. Versions were htalk 0.8.1, schema 3,
Codex CLI 0.157.1 and tmux 3.4. The worker used GPT-6 Luna at medium effort
through its existing ChatGPT login, with normal native permission checks.

## Observations

- One bootstrap turn returned READY and completed before either request.
- During a 72.4-second idle window, the worker's native history and reported
  token counters were unchanged. The same native PID and process start time
  remained present.
- The PC sent one saved UUID/body through SSH/MCP. The existing adapter
  returned a confirmed `codex_cli_queued` receipt. A new turn began in the
  same TUI, read and ACKed the exact request, computed 19 group aggregates
  from 4,096 synthetic records, and replied to that request.
- The PC independently verified every aggregate and all 1,595 result bytes
  before acknowledging the reply. The computation used one exclusive start
  marker, a 30-second CPU limit and a 512 MiB address-space limit.
- After the turn completed, a second 35.9-second idle window again showed
  unchanged native history, reported usage and process identity.
- A distinct follow-up request produced another queue receipt and another
  native turn in that same TUI. It read the existing result and replied without
  recomputing. Input, result, execution record and start-marker hashes stayed
  unchanged.
- There were exactly three worker turns: bootstrap and two incoming notices.
  The controller used no Enter, resume, manual turn start or TUI relaunch between
  the first send and the second completed reply. No htalk notification was
  retried. Each request and reply had its own ACK, four in total.
- Both request ACKs reported notification cleanup `absent` for their exact
  confirmed queue IDs. The notices had already been consumed.
- At completion both inboxes were empty. The owned TUI and its fixture
  processes were stopped, test peers retired and the temporary key revoked.
  A fresh connection with that key failed; original SSH access still worked.

Preparation included three unsuccessful fixture launches before a saved
thread existed. Captured errors came from temporary MCP argument escaping;
the successful run followed correction. A read-only observer also initially
assumed one thread per workspace. The native approval guardian had its own
row, so inspection was bound to the already registered exact worker UUID.
These preparation failures and their receipts were preserved, not replayed
as mailbox work. They were not htalk product changes.

## Limits

This establishes two idle-to-work transitions for this running TUI and client
version over one LAN. Unchanged reported usage describes only the two observed
idle windows; it is not a guarantee about every client background activity.
The computation ran on the laptop; model inference was cloud-hosted.

The result does not establish startup of stopped clients, waking a sleeping
device, offline mailbox placement, a general task-status protocol, Bluetooth,
WAN or other harnesses and operating systems. No Rust code, schema, dependency
or permanent automated test changed. The existing release supplies the route;
see [native setup](../../integrations/ssh.md#notify-an-already-running-codex-tui).
