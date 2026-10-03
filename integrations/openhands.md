# OpenHands notice admission and recovery

The launcher uses OpenHands CLI 1.16.0 / SDK 1.21.0 and the ordinary selected
TUI conversation. Model settings and tool approval remain native. Set
`HTALK_PEER`, `HTALK_DB` and `HTALK_BIN` as for that conversation's MCP tool.
Also set `HTALK_OPENHANDS_STATE` to a new private directory dedicated to this
conversation and mailbox. Keep `openhands_receiver.py` and
`managed_receiver.py` together. The state contains correlation metadata, not
message bodies; keep it private.

```sh
export HTALK_OPENHANDS_STATE=/absolute/private/openhands-admission
python3 -B "$HTALK_SOURCE/integrations/openhands_receiver.py" NATIVE_OPENHANDS_ARGUMENTS
```

The launcher's ordinary arguments are forwarded unchanged to OpenHands.
The ledger fixes the exact selected conversation ID, mailbox path, peer and
htalk executable. A changed binding is refused. A local lock prevents a second
launcher from sharing the ledger. The ledger is saved before notice admission.
A crash or forced stop leaves an active record and blocks the next launch
before the native CLI is imported or started.

Native admission schedules a Textual worker. It does not finish a turn. The
receiver waits for that exact worker and requires the same conversation runner
to report `finished`, with no active message/resume worker. A pause or pending
approval waits for the human; the receiver does not resume or approve it. An
error, changed conversation, cancelled worker or uncertain shutdown retains
the pending admission. Clean notice consumption advances the saved sequence,
so an ACKed request still awaiting another peer is not admitted again after a
clean restart. A later helper answer has its own sequence and can start the
next native turn. This cursor records notice consumption, not task completion.
The launcher never ACKs, sends or replies on the agent's behalf.

Inspect uncertain state without importing OpenHands or starting a child:

```sh
python3 -B "$HTALK_SOURCE/integrations/openhands_receiver.py" htalk-status \
  --state "$HTALK_OPENHANDS_STATE"
```

Stop any previous native work yourself. Inspect the pending mailbox message,
outgoing effects and the original native conversation history. The launcher
cannot determine whether interrupted external work completed. After that
inspection, choose an explicit disposition for the exact saved conversation
and pending notice:

```sh
python3 -B "$HTALK_SOURCE/integrations/openhands_receiver.py" htalk-recover \
  --state "$HTALK_OPENHANDS_STATE" --conversation SAVED_CONVERSATION_ID \
  --message PENDING_MESSAGE_ID --disposition consumed
```

`consumed` advances only the local notice cursor. Use it when the original
notice was consumed and should not be injected again, including when the
request is waiting for a helper. `retry` leaves the cursor unchanged and makes
that notice eligible on the next explicit launch. Choose it only after
reconciling prior effects. Neither choice modifies mail or native history,
starts inference, or proves that the task is done. A recovery receipt retains
the previous ledger. With no pending notice, omit `--message` and use
`consumed`; an active session still needs that explicit inspection decision.

The next run must select the same native conversation. To receive in a
different conversation, use a new dedicated state directory. Existing
pre-ledger sessions have no durable admission record. Inspect their history
and unfinished mailbox work before choosing that initial directory; the
launcher cannot reconstruct earlier notice consumption automatically.

The checked-in fixtures use a fake TUI/controller and fake SDK status. They
verify admission, completion, pause, crash/restart and binding guards without
providers, models or mailbox access. They do not establish native crash
behavior, exactly-once external work, or durable native history after abrupt
process loss.
