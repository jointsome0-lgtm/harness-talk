# Managed Letta session

`letta.py` runs a dedicated local Letta Code 0.33.0 agent through its headless
stream interface. It receives htalk requests and answers in one persistent
conversation. It requires Linux, Python 3.11 or later, and htalk 0.7.0 or later.
It does not attach to an existing Letta terminal or use Letta Cloud.

This adapter is included in the 0.8.0 source archive and checkout. Use a separate
test mailbox until its acceptance checks are complete. Keep `letta.py`,
`managed_receiver.py` and `letta/htalk.ts` together in the source checkout.
Goose and Letta share the notice, approval and recovery loop; mailbox commands
and their validation remain in the htalk MCP server.

## Prepare a dedicated profile

Use a fresh private directory. Do not copy a personal Letta profile into it or
open its agent in another process while the receiver is running. Configure the
provider through Letta using this profile. Model selection is the operator's
responsibility; receiving a notice can start paid inference.

Set `HTALK_SOURCE`, `HTALK_BIN` and `HTALK_DB` as in the
[shared setup](README.md#shared-setup). Use absolute paths throughout:

```sh
export LETTA_STATE=/absolute/private/letta-receiver
export LETTA_BIN=/absolute/path/to/letta
export HTALK_PEER=letta-worker
umask 077
mkdir -p "$LETTA_STATE/profile/home" "$LETTA_STATE/profile/work"

letta_profile() (
  cd "$LETTA_STATE/profile/work" || exit
  env HOME="$LETTA_STATE/profile/home" \
    XDG_CONFIG_HOME="$LETTA_STATE/profile/home/.config" \
    LETTA_HOME="$LETTA_STATE/profile/home/.letta" \
    LETTA_LOCAL_BACKEND_DIR="$LETTA_STATE/profile/backend" \
    DISABLE_AUTOUPDATER=1 LETTA_DISABLE_TELEMETRY=1 DO_NOT_TRACK=1 \
    "$LETTA_BIN" --backend local "$@"
)

letta_profile connect
letta_profile agents create --name htalk-worker \
  --model YOUR_CONFIGURED_MODEL_HANDLE --personality blank
```

Use the agent ID printed by the last command. Add only the shared htalk server
to that agent's local settings, preserving its other fields:

```sh
export LETTA_AGENT=agent-local-REPLACE_WITH_CREATED_ID
python3 - <<'PY'
import json, os
from pathlib import Path
profile = Path(os.environ["LETTA_STATE"]).resolve() / "profile"
path = profile / "home/.letta/settings.json"
settings = json.loads(path.read_text())
matches = [a for a in settings["agents"]
           if a["agentId"] == os.environ["LETTA_AGENT"]
           and a.get("baseUrl") == "local:" + str(profile / "backend")]
if len(matches) != 1 or matches[0].get("mcpServers"):
    raise SystemExit("Expected one new local agent without MCP servers")
matches[0]["mcpServers"] = [{"name": "htalk", "transport": "stdio",
    "command": str(Path(os.environ["HTALK_BIN"]).resolve()),
    "args": ["--db", str(Path(os.environ["HTALK_DB"]).resolve()),
             "--as", os.environ["HTALK_PEER"], "mcp"], "env": {}}]
path.write_text(json.dumps(settings, indent=2) + "\n")
PY

"$HTALK_BIN" --db "$HTALK_DB" peer add "$HTALK_PEER" \
  --harness generic --delivery pull
```

The receiver checks this registration against native `mcp list/get` before
starting a conversation. A different mailbox, peer, executable or extra server
stops startup. Its owned profile must have no saved tool allow rules or extra
mods. It installs the shipped `htalk.ts` mod there and refuses an edited copy.

## Run and restart

Write an [owner task file](README.md#owner-task-for-managed-sessions) before
launching the receiver. A blank agent has no task to execute on behalf of peers.

```sh
python3 -B "$HTALK_SOURCE/integrations/letta.py" run \
  --state "$LETTA_STATE" --agent "$LETTA_AGENT" \
  --db "$HTALK_DB" --peer "$HTALK_PEER" \
  --task-file /absolute/private/mail-task.txt \
  --htalk "$HTALK_BIN" --letta "$LETTA_BIN"
```

The terminal asks before each htalk call. `--allow-mail` explicitly approves all
shared htalk tool calls in this managed session, including sends. Other tools
are denied. Without that option, a noninteractive launch is refused. The mod
uses native MCP without a shell; htalk fixes identity and validates commands.

Repeat the same command after a clean exit to resume the saved conversation.
`--max-turns N` stops after N completed notice turns. One lock protects the
profile and state; the Letta child retains it if the controller dies.

Only one notice is dispatched at a time. A turn may read and ACK a request,
send a question to another peer, and finish while waiting. The peer's answer
starts another turn. The receiver advances its notice cursor only after
Letta reports a successful end of turn and the agent has ACKed the message.
This records notice handling, not task completion. The agent must reply to
the original request when its work is done.

## Inspect and recover

```sh
python3 -B "$HTALK_SOURCE/integrations/letta.py" status --state "$LETTA_STATE"
```

`status` only reads saved state. After an uncertain dispatch, `run` refuses to
load Letta. Inspect the pending message with `htalk show` and inspect outgoing
effects with `htalk sent`. Stop any remaining owned Letta process before
recovering; the inherited lock prevents concurrent recovery.

```sh
python3 -B "$HTALK_SOURCE/integrations/letta.py" recover \
  --state "$LETTA_STATE" --discard-session SAVED_CONVERSATION_ID \
  --message PENDING_MESSAGE_ID --disposition retry
```

`retry` makes the existing open notice eligible for a fresh conversation.
`settled` requires a saved reply for a pending request, or an ACK for a pending
answer. Neither disposition sends mail, ACKs it or starts Letta. If startup
stopped before selecting a message, omit `--message`, use `settled`, and pass
the saved conversation ID or `unknown` when no ID was saved.

The next explicit `run` creates a new conversation for the same agent. Its
previous conversation context is discarded; **agent memory and settings remain**.
Review other unfinished tasks before doing this. Local Letta memory remains
enabled; reflection is disabled. Clean shutdown waits for Letta's post-turn
memory synchronization. A forced stop leaves the receiver requiring inspection.

Routine receiver JSON contains state and correlation metadata, without message
bodies or model text. Manual approval prompts show the proposed arguments.
Native Letta diagnostics and its own transcripts follow Letta's logging rules.

## Verification on 2026-09-25

Native Letta with a local scripted provider read and ACKed a request, asked a
helper, stopped cleanly, resumed the same conversation, and used the helper's
answer to reply to the original request. All four linked mailbox rows were
acknowledged separately; the agent's memory files were unchanged.

A second native case held a turn, stopped the controller, and verified the
inherited lock and refusal to resume uncertain work. Explicit recovery created
a new conversation for the same agent and processed the original message
without sending it again. An unadvertised Bash call was denied before execution.
Both cases used source copies hashed before startup, isolated mailboxes and no
external model calls. Goose passed its delegated exchange and recovery checks
again after extraction of the shared loop.

A separate live Luna/Flex case completed the delegated exchange with an
explicit owner task and a synthetic helper. The same conversation resumed,
retained the original marker, and remained idle without inference until the
helper answered. All four linked rows were ACKed, eight model calls completed,
and owned processes exited. Memory files were unchanged. An out-of-scope peer
request to contact another recipient was ignored in this case; task scope is
still a model instruction, not a per-command policy.

Restart attempts with a missing or changed task file were refused before any
native process, inference or mail change. An earlier blank-agent case had only
read and ACKed its message: peer text alone did not provide an owner task.
