# Managed Goose session experiment

This throwaway Linux prototype checks whether one Goose 1.52.0 ACP session can
receive htalk requests and retain context across a clean restart. It owns a
separate session. It does not attach to an open Goose terminal.

## Run in a disposable workspace

Use a new scratch directory and the prototype branch. Configure Goose's provider
and model through its normal environment settings. Inference can incur charges.
The example leaves tool calls awaiting approval in this terminal.

```sh
export GOOSE_PATH_ROOT="$(mktemp -d)/profile"
export GOOSE_MODE=approve
export HTALK_BIN="$(command -v htalk)"
export GOOSE_BIN="$(command -v goose)"
mkdir -p "$GOOSE_PATH_ROOT/config" "$GOOSE_PATH_ROOT/work"
cp integrations/goose-prototype.yaml "$GOOSE_PATH_ROOT/config/config.yaml"
"$HTALK_BIN" --db "$GOOSE_PATH_ROOT/mail.sqlite3" peer add worker --harness generic --delivery pull
"$HTALK_BIN" --db "$GOOSE_PATH_ROOT/mail.sqlite3" peer add sender --harness generic --delivery pull
python3 -B integrations/goose-prototype.py \
  --state "$GOOSE_PATH_ROOT/state" --db "$GOOSE_PATH_ROOT/mail.sqlite3" \
  --workspace "$GOOSE_PATH_ROOT/work" --htalk "$HTALK_BIN" \
  --goose "$GOOSE_BIN" --peer worker
```

From another terminal, send a request to `worker` using this scratch database.
The worker prints each state transition, ACP update and permission decision.
`--max-turns 1` stops after one completed exchange. Restart the same command with
the same paths to load a cleanly saved session. Never copy a personal profile
into this experiment: stored sessions can retain their extension configuration.

`--allow-synthetic-mail` permits only show, ACK and reply for the current request
in a controlled test. It refuses tools after a reply is saved. The supplied
configuration disables Goose's default tools before session creation. Approve
mode alone does not remove them.

## Result on 2026-09-25

The native ACP check with canned model responses handled two distinct requests
in one session, restarted between them, and included the first task's context
in the second turn. Loading the clean session made no provider calls. Both
requests and replies were acknowledged separately.

The process lock survived through a stopped child after its parent was killed.
A second worker refused the lock. Once the child was stopped, recovery refused
to load the uncertain session or send another model request. Goose can resume
work during session loading, so a saved mailbox reply alone cannot authorize a
load after an interrupted turn.

An independent native negative probe advertised only htalk. Goose rejected
injected shell and write calls before execution. The tested profile and session
must be kept together for this restriction to hold.

The live Luna experiment remained incomplete. One route mismatch reached no
upstream provider. Two subsequent attempts encountered Flex provider errors,
including errors inside HTTP 200 streams. They consumed five upstream calls in
total, performed one mailbox read, and saved no ACK or reply. Their sessions
were preserved without automatic replay or provider fallback. Canned responses
verify session mechanics; they do not prove model reasoning or general task
execution.

## Decision and remaining work

Keep this branch as evidence for a dedicated managed-session receiver. Preserve
the refusal to load any interrupted dispatch until its outcome is reviewed.
Do not put this prototype in the 0.7.0 release.

A production version still needs a bounded recovery interface, native checks
for the remaining interruption points, and a completed live-model restart
experiment. Letta and Antigravity reuse depends on their own session loading and
permission behavior. No common worker framework is justified by this one case.
