# Native session checks with Luna, 2026-09-29

These Linux checks used the published htalk 0.9.6 x86-64 wheel from commit
`5c8955caf1a3b419d092ff7dc0c2cb9fdb0ce370`, SHA-256
`4abcfe1786432111d9cd17ab6895ca5922cc31d3fb6087217a25dccd907fdbc1`.
Adapter files came unchanged from that publication run's source archive.
Each harness used an isolated profile and mailbox. The operator started the
test sessions; htalk did not create or supervise agents.

Real GPT-6 Luna calls went through a bounded local gateway to OpenRouter's
OpenAI Flex route, with no model or provider fallback. The gateway kept the
credential outside the harnesses. Ordinary native tools performed the mailbox
operations; the controller independently checked saved replies and ACKs.
Arithmetic answers were computed by the model, not supplied by a canned
provider. Short idle observations establish only the periods stated below.

## Agent Zero WebUI

Agent Zero revision `e3051fb584b1a36be2b0a0c90606f1c2c2d356ec`, Python 3.12.3:

- Started the native WebUI on loopback with authentication, created one chat
  through its UI and bound the normal htalk plugin to that saved context.
- Restricted the profile's configurable tools to the htalk plugin. The native
  response tool remained available. Chat and utility model slots selected Luna.
- Submitted one bootstrap prompt through the UI. A subsequent mailbox notice
  woke the existing idle chat without another user prompt or manual resume.
- The agent read its inbox, showed the exact request, ACKed it separately,
  calculated `17 * 23`, and saved one correlated reply: `AZ-WEBUI 391`.
- The controller showed and ACKed the answer. Both mailbox rows were ACKed.
- The native chat became idle and unpaused. The reply was visible in the browser
  and remained visible after reloading the page and selecting the same chat.
- Nine model requests completed, including native utility work. Ten-second idle
  observations before mail and after browser reload made no extra model calls.
  One watcher was observed; the UI, gateway and watcher stopped during cleanup.

An earlier bootstrap click closed its browser before the request was accepted.
Readback found no saved user message or model call from that attempt. The
accepted attempt waited for the native API receipt; it was not a duplicate turn.

This closes the previous gap between a framework-only exchange and a visible
native WebUI exchange. It does not validate simultaneous user-send races,
multiple chats, media, document ingestion or a container deployment. The
development runtime's file-browser RPC was unavailable; the chat and WebSocket
path worked. The [documented native idle-reservation race](../../integrations/README.md#agent-zero)
still applies. No Agent Zero source patch was needed for this exchange.

## GitHub Copilot CLI

Copilot CLI 1.0.88 ran in its ordinary terminal with the released hooks and
native async Bash waiter. Only the bound MCP tool and exact waiter command
were permitted. After one bootstrap prompt, a native background-completion
notice woke the same session. The agent completed inbox, show, separate ACK
and one reply, `COPILOT-TUI 529`, for `23 * 23`. The controller independently
checked and ACKed it. The session rearmed one waiter. All eight model calls
completed, with no extra provider calls during five seconds of observation.

The exchange passed, but the runner's overall result remained incomplete:
`/exit` was still queued after its two-second cleanup deadline. The runner then
closed its owned terminal server. The session-end hook ran, receiver state
closed, and independent readback found no remaining owned processes. This
does not establish normal `/exit` behavior with Luna. The earlier canned-provider
idle/busy and normal-exit checks remain separate evidence.

## Gemini CLI

Gemini CLI 0.61.0 completed a real Luna exchange through the released TUI
launcher and a disposable external Gemini-to-OpenAI protocol bridge. The bridge
translated actual model outputs and preserved tool-call IDs; it supplied no
canned answers. Only the bound htalk MCP tool was allowed.

The initial ordinary terminal received a notice and completed inbox/show, but
the third provider call had an unknown outcome when the runner stopped. No ACK
or reply was saved. The bridge had waited for the complete upstream response
before sending HTTP headers. Its timeout exceeded Gemini's default headers
timeout; a retry arriving while that call was pending is the likely explanation
for the guard rejection, not a confirmed provider failure.

The test bridge was changed to send headers promptly, and the isolated Gemini
profile disabled automatic retries. An intermediate SSE-comment attempt failed
the installed SDK parser before tool execution. Attempts to resume the saved
session then produced malformed restored tool history: duplicate responses and
a response without its matching call. The bridge refused those payloads. The
exact origin of that malformed history remains unresolved.

An operator-started fresh native session used the original mailbox and request,
without another send. Its watcher recovered the pending request; the model
completed inbox, show, separate ACK and one reply, `GEMINI-TUI 629`, for
`17 * 37`. The controller independently checked and ACKed the answer. Five
provider calls completed, eight seconds of final idle added none, and `/exit`
closed the terminal normally. Independent readback found no owned processes;
both test peers were retired.

The fresh-session recovery passed. The interrupted first attempt and failed
same-session resumes remain failed trials. This establishes a real ordinary-TUI
exchange and pending-mail recovery in a fresh session, not transparent resume
of the old conversation. Protocol translation and model configuration remain
outside htalk; no installed Gemini source was modified.
