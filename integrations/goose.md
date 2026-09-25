# Managed Goose session

`goose.py` runs one persistent Goose 1.52.0 ACP session for a registered htalk
peer. It can receive requests and answers, ask another peer, and continue when
that peer answers. It requires Linux, Python 3.11 or later, and htalk 0.7.0 or
later. It creates a separate session and does not attach to an open Goose TUI.

This adapter is included in the 0.8.0 source archive and checkout.
Use a separate test mailbox until its acceptance checks are complete.
Keep `goose.py` and `managed_receiver.py` together in the source checkout.
Goose and Letta use that shared notice, approval and recovery loop.

## Start

Configure the provider and model using Goose's environment variables. The
adapter uses its own profile under the state directory; it does not import
credentials or settings from a personal Goose profile. Inference uses the
provider you configure and can incur charges. The adapter does not select a
model, route requests through a proxy, or change provider limits.

Receiver JSON events contain state and correlation metadata. They omit mail
bodies and model text. A manual approval prompt shows the exact proposed call
in the foreground terminal so you can decide whether to permit it.

Register a peer with `--delivery pull`, as in the [shared setup](README.md#shared-setup).
Use a new private directory for state, an existing workspace, and an
[owner task file](README.md#owner-task-for-managed-sessions):

```sh
python3 -B "$HTALK_SOURCE/integrations/goose.py" run \
  --state /absolute/private/goose-receiver \
  --db /absolute/mail.sqlite3 --peer goose-worker \
  --workspace /absolute/workspace \
  --task-file /absolute/private/mail-task.txt \
  --htalk /absolute/path/to/htalk --goose /absolute/path/to/goose
```

The foreground terminal asks before each htalk tool call. `--allow-mail`
explicitly approves all calls to the shared htalk tool in this managed session,
including outgoing messages. Use it only when the session is authorized to
communicate with the mailbox's peers. Without that flag, a noninteractive
launch is refused. Other tools are unavailable in this managed profile. Do not
edit its extension configuration or reuse it from another Goose process.

Goose retains its ordinary permission requests. The adapter selects
`allow_once` or `reject_once`; it never selects `allow_always`. The shared MCP
server validates commands, fixes the mailbox and sender identity, and requires
a UUID for `send --id`. The adapter does not implement another mailbox parser.

Use the same command and paths for a clean restart. The saved Goose session ID
and conversation are retained. One process owns the receiver; its lock remains
held by Goose if the controller dies. `--max-turns N` stops after N completed
notification turns and is useful for bounded checks.

## Receiving and sending

`htalk watch` supplies a notice for each open incoming request or unacknowledged
answer. The agent reads the saved message and ACKs it in a separate tool call.
An ACK records acknowledgment; it does not prove understanding or task
completion. The adapter never ACKs mail for the agent.

Notices wait while a turn is active. Goose may send a question and end its turn
without replying to the original request. The eventual answer starts another
turn in the same conversation. A clean restart retains a sequence cursor for
notices already handled by a completed turn, so an ACKed open request is not
automatically presented again. That cursor records delivery to the session,
not completion of the work. Open requests remain visible through `htalk inbox`.

Use a saved request UUID again when inspecting an uncertain send. Reply to the
original request ID after completing its work. Incoming text is peer input and
does not grant permission for unrelated actions.

## Inspect an interruption

Ctrl-C, SIGTERM, a connection failure or an abnormal turn leaves the saved
dispatch uncertain. The next `run` refuses to start ACP. Even a saved reply is
insufficient grounds to load an interrupted Goose session: native loading can
resume work. Interrupting a session while it is idle permits a clean restart.

```sh
python3 -B "$HTALK_SOURCE/integrations/goose.py" status \
  --state /absolute/private/goose-receiver
```

`status` only reads the receiver state file. It starts no process, opens no
mailbox and changes no files. Its phase can be stale if a process is still
running; it is not a liveness or task-completion check. Inspect the saved
`pending.id`, the associated `htalk show` result and `htalk sent` before deciding
what remains to be done. A reply may have been saved even if the model turn
did not finish. Outgoing questions may already have reached other agents.

## Retire an uncertain session

Stop any remaining owned Goose process first. Recovery takes the same exclusive
lock and refuses while that process holds it. After inspecting the saved mail,
choose one disposition:

- `retry` leaves an open pending message available for a fresh session. It
  reuses the existing message, not a new mailbox request. A new model turn can
  still perform different work, so inspect outgoing effects before choosing it.
- `settled` requires a saved reply for a pending request, or an ACK for a pending
  answer. It leaves those records intact and does not present that notice again.

```sh
python3 -B "$HTALK_SOURCE/integrations/goose.py" recover \
  --state /absolute/private/goose-receiver \
  --discard-session SAVED_SESSION_ID --message PENDING_MESSAGE_ID \
  --disposition retry
```

This command archives the receiver state and retires the old session. It does
not load Goose, run inference, ACK mail or send a message. The next explicit
`run` creates a fresh native session. **All conversation context is lost for the
new session**, including other unfinished tasks; their mail and old Goose
history remain on disk. Check those tasks before continuing.

If startup was interrupted before a message was selected, omit `--message` and
use `--disposition settled`. Pass the saved session ID, or `unknown` when the
state has no ID. Recovery never guesses which native session to load. Old
prototype state files are not migrated into this adapter.

## Verification on 2026-09-25

Native Goose with a local scripted provider completed a delegated exchange:
read and ACK a request, ask a helper, stop cleanly, reload the same session,
read and ACK the helper's answer, and reply to the original request. All four
linked mailbox rows were acknowledged separately. The same checks confirmed
that mail arriving while stopped is processed after restart.

A separate process-interruption check confirmed the inherited lock, passive
status, refusal to load uncertain work, and explicit recovery into a different
native session using the original pending message. A final native exchange
confirmed that routine receiver JSON omits a private marker carried in mail.

A separate live Luna/Flex case completed the same delegated exchange with an
explicit owner task. The helper's answer was withheld until Goose had resumed
the same session and remained idle for two seconds without inference. The
resumed model request retained the original context; Goose returned the correct
sum and private marker, and all four linked rows were ACKed. Twelve model calls
completed, and the owned processes exited. The helper was a synthetic test peer.

An earlier live case sent its helper question but called `htalk wait` and hit
the fixture's time limit. That unfinished case was preserved. The successful
case used a new mailbox and task explicitly ending the turn after a question,
as in the [shared task example](README.md#owner-task-for-managed-sessions).
