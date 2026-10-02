# OpenCode sessions

Checked on 2026-10-01 UTC with the htalk `0.10.0` publication wheel,
commit `0498932f887865737a6df887773c76c14954dd51`, SHA-256
`3bdd20ad5c0af3704b55948efbf097b3bd03832c187229cc16c331405a22038c`.
OpenCode 1.18.32 completed native MCP `show`, separate `ack`, and one correlated
`reply` in a fresh server session with a local canned provider. Its request and
reply were ACKed, both final inboxes were empty, and the owned server stopped.
The fixture kept the wheel environment first on PATH, used the source archive
from the same publication run, and allowed only the htalk MCP tool.
ACK cleanup was `unsupported` for OpenCode and `skipped/pull_only` for the
synthetic helper. This checks transport and native tool execution with no
external model calls. See the [publication check](checks/2026-10-01-catalog.md).

Checked on 2026-09-29 UTC with the htalk `0.9.6` x86-64 publication wheel,
commit `5c8955caf1a3b419d092ff7dc0c2cb9fdb0ce370`, SHA-256
`4abcfe1786432111d9cd17ab6895ca5922cc31d3fb6087217a25dccd907fdbc1`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36506294790)
passed both Linux architecture builds and installed-wheel checks. All 100
included Git files matched the release tree. The three PyPI files and a fresh
installation matched the checked artifacts.

OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Tools were denied by default, with only the bound htalk MCP tool
allowed. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, peers retired, and the owned server process stopped.
Native cleanup was `unsupported`; controller cleanup was `skipped/pull_only`.
This verifies the server session and native MCP execution. TUI attachment,
model reasoning and notification removal were not established. No external
model call was made for this OpenCode check.

Checked on 2026-09-28 UTC with the htalk `0.9.5` x86-64 publication wheel,
commit `709e5ea8f855ca19296203f65a01bc94fb2b066a`, SHA-256
`53f179a5ab74686d9acfe25e7ed0dc8069e5753f362498667db8492f0b9241b6`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36482432457)
passed both Linux architecture builds and installed-wheel checks. All 98 included
Git files matched the release tree. The three PyPI files and a fresh installation
matched the checked bytes.

OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Tools were denied by default, with only the bound htalk MCP tool
allowed. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, peers retired, and the owned process group was absent
after shutdown. Native cleanup was `unsupported`; controller cleanup was
`skipped/pull_only`. This verifies the server session and native MCP execution.
TUI attachment, model reasoning and notification removal were not established.
No external model call was made for this OpenCode check.

Checked on 2026-09-28 UTC with the htalk `0.9.4` x86-64 publication wheel,
commit `cab29eabc7d2671bdb9dfacfddb4ad6303459a05`, SHA-256
`89494cd0b9b7e6f18cdb3fd13fff7780df0e90e89347979bde8bb0a994844649`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36429231142) passed both Linux architecture builds and
installed-wheel checks. All 97 included Git files matched the release tree.
The three PyPI files and a fresh installation matched the checked bytes.

OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Tools were denied by default, with only the bound htalk MCP tool
allowed. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, peers were retired, and the owned process group was
empty after shutdown. Native cleanup was `unsupported`; controller cleanup was
`skipped/pull_only`. This verifies the server session and native MCP execution.
TUI attachment, model reasoning and notification removal were not established.
No external model call was made for this OpenCode check.

Checked on 2026-09-28 UTC with the htalk `0.9.3` x86-64 publication wheel,
commit `e98701dbb2c01194254341603f5f8290afddceee`, SHA-256
`49cf45b55ec31d8bc17852827cbf74d1098e731e8b94f5f977c215cd098829bc`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36360798674) passed both Linux architecture builds and
installed-wheel checks. All 96 included Git files matched the release
tree. Published PyPI files and a fresh installation matched the checked bytes.

OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Tools were denied by default, with only the bound htalk MCP tool
allowed. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, fixture peers were retired, and the owned server was
stopped. Native cleanup was `unsupported`; controller cleanup was
`skipped/pull_only`. This checks the server session and native MCP execution,
not TUI attachment, model reasoning or notification removal. No external model
call was made for this OpenCode check.

Checked on 2026-09-27 UTC with the htalk `0.9.2` publication wheel from
commit `4b8c27cb848a75f33cf4edd0ade15fa320c0fccc`, SHA-256
`bf3e6667f383fd7f82f942dccd5893d21f3627d93ee2e7a6faad5c092cfba75c`.
OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Permissions denied tools by default and allowed the bound htalk MCP
tool. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, both peers were retired, and the owned process group
was absent after shutdown. Native cleanup was `unsupported`; controller cleanup
was `skipped/pull_only`. This checks the server session and native MCP execution,
not TUI attachment, model reasoning or notification removal. No external model
call was made. The wheel and matching source archive came from
[publication run 36314173772](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36314173772)
and were checked before upload approval.

Checked on 2026-09-27 UTC with the htalk `0.9.1` publication wheel from
commit `ab5a9e897cc176c67b480a3ba98baa0c8b0588d2`, SHA-256
`8ba0e59a798448d69a44df83537c64fc59cdd546ffc4fe5c9e4959ca891c5cf1`.
OpenCode 1.18.32 used an isolated loopback server session and a local canned
provider. Permissions denied tools by default and allowed the bound htalk MCP
tool. One native notice led to show, separate ACK and one correlated reply;
the controller read and ACKed the answer. Four canned provider calls completed,
both inboxes were empty, both peers were retired and the owned process group
was stopped. Native notification cleanup was `unsupported`; controller cleanup
was `skipped/pull_only`. This checks the server session and native tool path,
not TUI attachment, model reasoning or notification removal. No external model
call was made. The same-run wheel and source archive were checked before upload
approval for [publication run 36308690259](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36308690259).

Checked on 2026-09-27 UTC with the htalk `0.9.0` publication wheel from
commit `5e20371fde81e85dec06727f7be84f162aefb14f`, SHA-256
`efb3d73970254d3be155f96e616b75603847dd34c63b62997a0928b4b6e5bf92`.
OpenCode 1.18.32 ran an isolated loopback server session with a local canned
provider. Permissions denied all tools except the bound htalk MCP tool.

One native request notice produced three completed tool calls: show, separate
ACK, and one reply. The controller read and ACKed the correlated answer. Four
canned provider calls completed, both inboxes were empty, both peers were
retired, and the owned server process group had no remaining processes.
Native ACK cleanup was `unsupported`; controller cleanup was
`skipped/pull_only`. This checks server-session transport and native tool
execution, not ordinary TUI receiving or model reasoning.

An earlier fixture attempt failed when the sandbox denied its loopback bind,
before a server, mailbox or send existed. That attempt was preserved. The
passing check used approved host execution in a new directory with a fresh ID.
The wheel came from [publication run 36292771193](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36292771193)
and was checked before upload approval.

Checked on 2026-09-26 UTC with the htalk `0.8.1` publication wheel, commit
`ca2e237a054d79b051a34962cff05b5d0178477f`, SHA-256
`b5aed6d76e73e479c4a257df1e4a5924f815295b219fe4516e8268a56b77a399`.
OpenCode 1.18.32 used an isolated profile, a native loopback server session and
local canned responses. Tools were denied by default with only `htalk_*` allowed.
One native notice led to three completed MCP calls: show, separate ACK and reply,
then final native output. The controller read and ACKed the correlated
`OPENCODE 42` reply. Both rows had ACKs; four canned provider calls completed
without a guard error, and the owned server process group was stopped.
The native ACK reported `unsupported/client_has_no_notification_removal`;
the controller ACK receipt separately recorded `skipped/pull_only`.
The published wheel matched the checked file. This verifies a server session,
not TUI attachment, real model reasoning or notice removal. No external model
call was made.

Checked on 2026-09-26 with the htalk `0.8.0` publication wheel, commit
`8a07ffc22b38a006404b64eb7c7b0e0a62689c40`, SHA-256
`fce59255b26a07dad0a521adcd5cfcb90fc2100313d79a9f04cd70ffd615996d`.
OpenCode 1.18.32 used a native loopback server session, an isolated profile and
a local canned provider. Permissions denied tools by default and allowed only
`htalk_*`. One native HTTP notice started a turn; show, separate ACK and reply
completed through MCP, followed by final native output. The controller read
and ACKed the correlated `OPENCODE 42` reply. Both rows had ACKs, four canned
provider calls completed without a guard error, and the owned process group
was stopped. No external model call was made. This verifies a server session,
not attachment to an ordinary TUI. The retained native ACK output reports
`unsupported/client_has_no_notification_removal`. The controller's reply ACK
is present in the mailbox, but its cleanup receipt was not retained. ACK storage,
notification removal and process cleanup are separate observations.

Checked on 2026-09-25 with the htalk 0.7.0 publication wheel, commit
`c18c87b3c0b654f32e1dca127fdb1d8c2aba3a48`, SHA-256
`f97968e1bb8dbca6b8e49dee39889f1e95fb74cf2ec3d6d6de9ebcf993baf232`.
OpenCode 1.18.31 ran a native loopback server session with a local canned
provider and only the fixture htalk MCP tool permitted. A saved request's HTTP
notice started a turn; native show, ACK and reply calls produced the correlated
answer. The controller read and acknowledged that answer. Both rows had ACKs,
and the owned server process group was stopped. OpenCode's native ACK returned
`notification_cleanup: unsupported` with
`client_has_no_notification_removal`. The controller's ACK was verified in
storage, but its cleanup output was not retained. Neither a stored ACK nor
process shutdown proves native notice removal. This checks a server session,
not attachment to an ordinary TUI or real model reasoning. The older checks
below cover their stated versions and model routes.

Checked on 2026-09-23 with the htalk `0.6.1` x86-64 publication wheel from commit `6b6b9efef90455ecff640049f38d4e14f15ace37`, SHA-256 `89befb955c10b8b49796bfcdae3f3b2e03fabc98dd84d192dd2858aeaba84e75`. Its published PyPI file has the same hash. A headless OpenCode `1.18.31` session used `opencode/muse-spark-1.3-contributor-free`, an isolated schema-3 mailbox and a loopback server. Built-in tools retained ordinary `ask` permissions, with six exact fixture helper commands allowed.

OpenCode and a pull participant each initiated one request and received its correlated reply. All four messages were shown and acknowledged in separate commands, with each request acknowledged before its reply. Both native notices appeared once and led to the expected tool calls; neither pull message attempted a notification. Both final inboxes were empty, both peers were retired and the server exited. Native cleanup was `unsupported`; pull cleanup was `skipped/pull_only`. Nothing was resent or renotified. Live provider metadata listed zero prices and every recorded assistant message used the intended model with reported cost zero. This checks OpenCode ↔ pull on Linux.

Checked on 2026-09-23 with the htalk `0.6.0` x86-64 publication wheel from commit `9963bb4dee3eeb1a6b63ea64978b34dd1977e109`, SHA-256 `c70f8990a8d66cecad7f96747f6f6dacaaa9721f5532ab8725ef6c42579ad23b`. A headless OpenCode `1.18.31` session used `opencode/muse-spark-1.3-contributor-free` with an isolated schema-3 mailbox. Built-in tools kept ordinary `ask` permissions; six exact test helper commands were allowed. The first server launch failed inside the test runner's sandbox before a listening endpoint, session, prompt or mailbox existed. A separate host setup completed the exchange; the first `ServeError` did not establish its precise cause.

OpenCode and a pull participant each initiated one request and received a correlated reply. All four messages were shown and acknowledged in separate commands, with acknowledgment preceding each reply. Both native notices appeared once in the OpenCode transcript and led to the expected tool calls; the two pull messages made no notification attempt. Both final inboxes were empty, the peers were retired and the test server exited. Native cleanup remained `unsupported`; pull cleanup was `skipped/pull_only`. Nothing was resent or renotified. Live provider metadata listed zero prices, and every recorded assistant message used the intended model with reported cost zero. This checks OpenCode ↔ pull on Linux, not direct OpenCode ↔ Codex/Claude or another device. The wheel was checked before PyPI approval; its published file has the same hash.

Verified on 2026-09-10 with htalk `0.3.0.dev0` and a local headless OpenCode `1.18.30` server. A public synthetic exchange used `opencode/muse-spark-1.3-contributor-free`: OpenCode read and acknowledged the request through htalk, replied, initiated a reverse request, and read and acknowledged the driver's answer. Both notifications were accepted once; the native trace recorded both completed turns and the matching htalk commands. The other peer was a local test driver, so this checks OpenCode's participation rather than another harness's wakeup behavior. OpenCode was not installed for the htalk 0.4.0 release check on 2026-09-17, the Rust-based 0.5.0 check on 2026-09-21, or the 0.5.1 check on 2026-09-22 UTC. None of those releases has a new live OpenCode observation. The 0.5.1 release passed its synthetic adapter contracts and installed CLI tests on Linux x86-64 and ARM64; those do not replace a native-client check.

The routes below were read from that server's OpenAPI document at `GET /doc` and the [official server documentation](https://opencode.ai/docs/server/). Health, session lookup, status, authentication and unknown-session rejection were also checked directly without a model.

## Transport

OpenCode runs an HTTP server whether started as `opencode` (TUI plus server) or `opencode serve`. The documented default address is `http://localhost:4096`. `htalk` talks only to an explicitly registered loopback URL; it never starts, attaches to, or discovers a server from process state, because the instance lock files under `$XDG_STATE_HOME/opencode/locks` record a PID but no port.

Reads are `GET /global/health`, `GET /session/SESSION_ID?directory=WORKSPACE` and `GET /session/status?directory=WORKSPACE`. Delivery is one `POST /session/SESSION_ID/prompt_async?directory=WORKSPACE` with `{"parts": [{"type": "text", "text": ...}]}`, which the server answers with `204` before the session processes the text. The `directory` query selects the project instance. The server omits idle sessions from the status map, so absence means `idle`; `busy` and `retry` are reported as returned.

## Registration

```sh
htalk peer add muse --harness opencode --session ses_XXXXXXXX \
  --workspace /absolute/project --url http://127.0.0.1:4096
```

OpenCode session identifiers are opaque strings beginning with `ses`; they are not UUIDs and are stored as given. `--url` is optional and defaults to the documented address; it must be an `http` or `https` URL whose host is `localhost` or a literal loopback IP address, without credentials, query or fragment. Names such as `127.attacker.example` are rejected, so an environment-provided password can only ever be sent to the local machine. Resolver results are filtered before connecting; only loopback IP addresses are tried. Addresses remain immutable and unique per harness and session. `--socket` is rejected for OpenCode, and `--url` is rejected for other harnesses.

Opening a database first checks its schema in a read transaction. A complete current schema needs no writer lock. New databases use schema 3. Version 0.6.1 automatically backs up and upgrades schema 1 or 2 on first use; version 0.6.0 requires explicit migration. See [migration and recovery](reference.md#database-and-peers). Update every installation sharing the mailbox: clients 0.5.1 and earlier reject schema 3.

## Authentication

When the server was started with `OPENCODE_SERVER_PASSWORD`, every route including health answers `401` without HTTP basic credentials. `htalk` reads `OPENCODE_SERVER_PASSWORD` and `OPENCODE_SERVER_USERNAME` (default `opencode`) from its own environment at request time, exactly as the server does. The password is never written to the peer row, results, notification text or logs. `peer check` reports `authenticated: true` only to say credentials were sent; a `401` is reported as `opencode_unauthorized`.

## Check and notify

`peer check` requires a healthy server, the exact session on the registered workspace, and an unarchived session; it reports `runtime_status` (`idle`, `busy` or `retry`), `server_version` and `transport: opencode_http_prompt_async`. Failures are fixed codes: `opencode_unreachable`, `opencode_unauthorized`, `recipient_not_in_opencode_server`, `recipient_identity_changed`, `recipient_session_archived`, `opencode_invalid_response`.

Notification repeats that check, rereads the saved message state, then makes at most one `prompt_async` request. Cancellation is checked after preflight and again immediately before writing the HTTP request, including after TCP and TLS setup. When either check observes Ctrl-C, no POST is sent; the saved message keeps its durable unknown claim and is never replayed automatically. Cancellation can still race with the write after the final check. If the message was acknowledged during preflight, no POST is sent and the result is `not_submitted` with `acknowledged_before_notification`; an answer the requester's `wait` has already returned is likewise not posted, with `returned_by_recipient_wait`. Any preflight failure, a connection failure before the request is written, or a completely received `4xx` answer is `not_submitted`, because the server states that nothing was accepted. A non-JSON error body does not change that rejection. A `204` is `submitted` with detail `opencode_prompt_async_accepted`. A timeout, a closed connection, invalid or incomplete HTTP framing, an unreadable successful response or a `5xx` after the request was written is `submission_unknown`, and the store's claim prevents any replay. Acceptance means the server queued the text for that session; it does not prove the model read it. A reply or an explicit acknowledgment establishes progress, as for the other harnesses.

The notification points to `show` for the exact message ID, as for every harness. A `busy` session can receive it within its current tool loop. On 2026-09-10, OpenCode `1.18.30` read a reply through `htalk wait`, received the native notice before acknowledging it, and completed the current turn without a later extra turn. An idle session starts a turn with whatever model and agent the session already uses. `htalk` passes no model, agent, tools or system prompt.

A separate controlled case delayed preflight until after acknowledgment and the current turn's final response. Before the final acknowledgment check was added, the resulting POST produced a second native model turn for the stale notice. With the check, a new case made no notification POST, stayed idle and retained one final response during the following minute. Each case used one synthetic answer with no replay. These observations cover that ordering, not every possible race.

Acknowledgment after the final check can still race with delivery. The inspected server exposes message-history deletion and session abort, but no pending-notification cancellation operation. htalk does not delete native conversation history or abort a session to hide an acknowledged notice. Cleanup after submission remains unsupported.

## Discovery

Use the read-only CLI discovery command:

```sh
htalk peer discover --harness opencode --opencode-url http://127.0.0.1:4096 \
  --workspace /absolute/project
```

Repeat `--opencode-url URL` to inspect several servers. Omit `--workspace` to query each server's own project. Discovery uses only GET requests. For each URL it lists sessions (`GET /session`) with their server-reported status (`GET /session/status`); with `--workspace PATH` both requests carry `directory=PATH` so the intended project is queried. Child sessions (`parentID`) and archived sessions are skipped. It then reads the local metadata database at `$XDG_DATA_HOME/opencode/opencode.db` (default `~/.local/share/opencode/opencode.db`, the path printed by `opencode db path`) in read-only mode, taking the 50 most recently updated unarchived root sessions. Saved sessions carry `runtime_status: "unknown"` with `runtime_reason: "saved_metadata_only"` and no URL: a saved session is not evidence of a running server. Titles, messages and credential tables are never read.

Every source entry carries `harness: "opencode"`, a `status` of `ok`, `partial` or `unavailable`, an `error` code and a `detail` code. A malformed or non-loopback URL makes only its own source `unavailable` with `invalid_opencode_url` or `opencode_url_must_be_loopback`, reported without the submitted text; other URLs and the saved metadata are still read. Unreachable, unauthorized, oversized (`opencode_response_too_large`, above 4 MiB) or malformed answers are `unavailable` with a fixed code. When more saved sessions exist than the limit, the saved source is `partial` with `opencode_saved_session_limit_reached`; a missing file is `unavailable` with `opencode_saved_metadata_missing`.

An invalid session record or saved address row is skipped individually. Its source becomes `partial` and includes a `rejected` count, while valid addresses before and after it are preserved. Saved IDs and directories must be valid UTF-8 text; an invalid UTF-8 cell in either field rejects only that row, with `opencode_invalid_saved_metadata` in `detail`. Only integer `time_updated` values become timestamps. Every other type, including malformed UTF-8 TEXT, gives `updated_at: null` and preserves an otherwise valid address.

The saved-row limit applies to the first 50 rows selected by SQLite's `ORDER BY time_updated DESC`, before validation. Rejected rows are not replaced with older rows. A 51st row is used only to detect the limit; its cells are not decoded and it does not contribute to `rejected`. If the limit and row rejections both apply, `detail` remains `opencode_saved_session_limit_reached` and `rejected` counts the invalid selected rows. Database open, schema, query or read failures still make the saved source `unavailable`, with a fixed error such as `opencode_saved_OperationalError` or `opencode_saved_DatabaseError`. A malformed server response also makes that server source unavailable.

Limitations: a server lists only the sessions of the directory it serves, so sessions of other projects appear through saved metadata only; a session served by a server on another port is unknown until that URL is supplied; the saved schema is the one observed in `1.18.30` and may change.
