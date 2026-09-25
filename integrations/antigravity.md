# Managed Antigravity SDK session

`antigravity.py` receives htalk notices in a dedicated Antigravity SDK 0.1.18
conversation. It uses the same notice, lock, cursor and recovery module as
Goose and Letta. The SDK runs the agent and calls the shared htalk MCP server.
This candidate is under development and is not included in htalk 0.7.0.
It requires Linux, a Python 3.11+ controller and a separate Python environment
with the pinned SDK. It does not attach to an Antigravity CLI or IDE window.

## Setup

Use a fresh test mailbox and private state directory. Keep `antigravity.py`,
the `antigravity/` directory and `managed_receiver.py` together in the checkout.
Set `HTALK_SOURCE`, `HTALK_BIN` and `HTALK_DB` as in the
[shared setup](README.md#shared-setup), using absolute paths.
Write an [owner task file](README.md#owner-task-for-managed-sessions) to define
what work this session may do with incoming peer messages.

```sh
export AGY_SDK_ENV=/absolute/private/antigravity-sdk
export AGY_STATE=/absolute/private/antigravity-receiver
export HTALK_PEER=antigravity-worker
uv venv --python 3.13 "$AGY_SDK_ENV"
uv pip install --python "$AGY_SDK_ENV/bin/python" google-antigravity==0.1.18

"$HTALK_BIN" --db "$HTALK_DB" peer add "$HTALK_PEER" \
  --harness generic --delivery pull

python3 -B "$HTALK_SOURCE/integrations/antigravity.py" run \
  --state "$AGY_STATE" --sdk-python "$AGY_SDK_ENV/bin/python" \
  --db "$HTALK_DB" --peer "$HTALK_PEER" --htalk "$HTALK_BIN" \
  --model YOUR_CONFIGURED_MODEL --base-url http://127.0.0.1:PORT/v1 \
  --task-file /absolute/private/mail-task.txt \
  --allow-mail
```

Supply an existing local OpenAI-compatible endpoint. The adapter does not
install, configure or translate the provider. Credentials belong to that
endpoint; the SDK receives no provider key from this launcher. URLs with
credentials, a query or a fragment are refused. Receiving a notice can start
inference. The operator chooses the model and pays any provider charges.

SDK 0.1.18 currently emits `tool_choice: "none"` even when it advertises the
MCP tools in the tested configuration. Live Luna verification required an
external provider gateway that changed this field to `auto`. Such a gateway
is not shipped here; the tested Luna route requires that provider adaptation.

`--allow-mail` is required. It authorizes all shared htalk operations, including
sends, in this session. There is no per-call approval prompt. The native SDK
policy denies tools by default and allows only the bound server's `htalk`
tool. Built-in tools, subagents, custom tools, skills and triggers are disabled.
The SDK host has a private HOME, workspace, session store and application data.
This is process configuration, not an OS sandbox.

## Restart and recovery

Repeat the same run command after a clean exit. The SDK uses `CREATE_ONLY` for
a new caller-chosen UUID and `RESUME` for an existing one. The SDK reports the
observed conversation ID after its first turn; the receiver requires it to
match the configured ID. `--max-turns N` closes after N handled notices.

The SDK's stock OpenAI convenience configuration drops continuation mode and
policies when creating its connection. `receiver_config.py` uses the SDK's
configuration factory to pass them explicitly. It requires exactly 0.1.18 and
does not patch the installed package. Provider behavior remains native.

The SDK host process retains the receiver lock. The compiled native runtime
does not inherit that descriptor; it runs in the host's process group. Clean
shutdown closes the SDK context, while forced cleanup stops that group.
ACK records notice handling. An agent waiting for another peer can finish a
turn and handle the answer later; it must reply to the original request when
the task is complete.

```sh
python3 -B "$HTALK_SOURCE/integrations/antigravity.py" status --state "$AGY_STATE"

python3 -B "$HTALK_SOURCE/integrations/antigravity.py" recover \
  --state "$AGY_STATE" --discard-session SAVED_CONVERSATION_ID \
  --message PENDING_MESSAGE_ID --disposition retry
```

`status` starts no child. After interruption, inspect the pending message and
outgoing mail, then stop any remaining owned SDK processes. The common recovery
command retires the conversation without starting a model or modifying mail.
Use `settled` only for a request with a saved reply or an ACKed answer. If there
is no pending message, omit `--message` and use `settled`. An interrupted
startup without a saved ID requires `--discard-session unknown`.

The next explicit run creates a fresh conversation. Old native session files
remain for inspection. See [Goose recovery](goose.md) for the shared lifecycle.

## Verification

On 2026-09-25, this receiver completed a delegated htalk exchange across
strict same-ID resume using the native SDK and a local scripted provider.
Both native turns completed, four correlated rows were ACKed, and the resumed
model request contained the original request and helper question. The observed
idle interval made no model calls; both SDK hosts and native children closed.
This checks native tools and persistence; scripted responses do not establish
model reasoning.

A separate live Luna/Flex case with an explicit owner task completed the same
exchange with a synthetic helper. It resumed the same native conversation,
retained the original private marker, and produced four linked ACKed rows.
The observed idle interval made no model calls; all owned processes exited.
Nine upstream calls completed through the external gateway described above.

The SDK exposes a generic `call_mcp_tool` wrapper. The adapter supplies a short
htalk usage instruction with the wrapper fields and bound-identity arguments.
Without it, an earlier case spent its 14-call budget learning tool syntax and
stopped before handling the helper answer. That case remains a failed attempt.

Separate native checks covered restart before any model turn, rejection of a
second writer, interruption with a saved pending message, passive status and
refusal to resume uncertain work. Explicit recovery left mail unchanged and
the next run used a fresh conversation without retired history.
A scripted `run_command` call was rejected as an unknown tool before execution;
the scratch canary file was absent and the notice remained unACKed. This checks
the restricted native tool surface, not the separate policy-decision path.

The ordinary CLI separately passed a persistent stdin and restart check, but
its tested restricted custom agent still exposed background-task control.
That CLI configuration is not the implementation used here.

Sources: [SDK](https://github.com/google-antigravity/antigravity-sdk-python),
[persistence](https://github.com/google-antigravity/antigravity-sdk-python/blob/v0.1.18/examples/getting_started/persistence.py),
[native connection](https://github.com/google-antigravity/antigravity-sdk-python/blob/v0.1.18/google/antigravity/connections/local/local_openai_connection.py),
[receiver configuration factory](antigravity/receiver_config.py).
