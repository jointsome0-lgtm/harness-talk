# OpenCode sessions

Verified on 2026-09-10 with htalk `0.3.0.dev0` and a local headless OpenCode `1.18.30` server. A public synthetic exchange used `opencode/muse-spark-1.3-contributor-free`: OpenCode read and acknowledged the request through htalk, replied, initiated a reverse request, and read and acknowledged the driver's answer. Both notifications were accepted once; the native trace recorded both completed turns and the matching htalk commands. The other peer was a local test driver, so this checks OpenCode's participation rather than another harness's wakeup behavior.

The routes below were read from that server's OpenAPI document at `GET /doc` and the [official server documentation](https://opencode.ai/docs/server/). Health, session lookup, status, authentication and unknown-session rejection were also checked directly without a model.

## Transport

OpenCode runs an HTTP server whether started as `opencode` (TUI plus server) or `opencode serve`. The documented default address is `http://localhost:4096`. `htalk` talks only to an explicitly registered loopback URL; it never starts, attaches to, or discovers a server from process state, because the instance lock files under `$XDG_STATE_HOME/opencode/locks` record a PID but no port.

Reads are `GET /global/health`, `GET /session/SESSION_ID?directory=WORKSPACE` and `GET /session/status?directory=WORKSPACE`. Delivery is one `POST /session/SESSION_ID/prompt_async?directory=WORKSPACE` with `{"parts": [{"type": "text", "text": ...}]}`, which the server answers with `204` before the session processes the text. The `directory` query selects the project instance. The server omits idle sessions from the status map, so absence means `idle`; `busy` and `retry` are reported as returned.

## Registration

```sh
htalk peer add muse --harness opencode --session ses_XXXXXXXX \
  --workspace /absolute/project --url http://127.0.0.1:4096
```

OpenCode session identifiers are opaque strings beginning with `ses`; they are not UUIDs and are stored as given. `--url` is optional and defaults to the documented address; it must be an `http` or `https` URL whose host is `localhost` or a literal loopback IP address, without credentials, query or fragment. Names such as `127.attacker.example` are rejected, so an environment-provided password can only ever be sent to the local machine. Addresses remain immutable and unique per harness and session. `--socket` is rejected for OpenCode, and `--url` is rejected for other harnesses.

Opening a database runs one immediate write transaction that creates the tables, adds the nullable `peers.url` column to a 0.2 file if it is missing, and sets schema version 2. Concurrent first opens serialize on that transaction, so the column is added once. Messages, acknowledgments and addresses are preserved; existing peers read back with `url: null`. A 0.2 client refuses a version 2 file with `unsupported_database_version` instead of failing on its five-column insert, and this version refuses anything newer.

## Authentication

When the server was started with `OPENCODE_SERVER_PASSWORD`, every route including health answers `401` without HTTP basic credentials. `htalk` reads `OPENCODE_SERVER_PASSWORD` and `OPENCODE_SERVER_USERNAME` (default `opencode`) from its own environment at request time, exactly as the server does. The password is never written to the peer row, results, notification text or logs. `peer check` reports `authenticated: true` only to say credentials were sent; a `401` is reported as `opencode_unauthorized`.

## Check and notify

`peer check` requires a healthy server, the exact session on the registered workspace, and an unarchived session; it reports `runtime_status` (`idle`, `busy` or `retry`), `server_version` and `transport: opencode_http_prompt_async`. Failures are fixed codes: `opencode_unreachable`, `opencode_unauthorized`, `recipient_not_in_opencode_server`, `recipient_identity_changed`, `recipient_session_archived`, `opencode_invalid_response`.

Notification repeats that check, rereads the saved message state, then makes at most one `prompt_async` request. If the message was acknowledged during preflight, no POST is sent and the result is `not_submitted` with `acknowledged_before_notification`; an answer its requester is polling for with `wait` is likewise not posted, with `recipient_waiting_for_this_answer`. Any preflight failure, a connection failure before the request is written, or a `4xx` answer is `not_submitted`, because the server states that nothing was accepted. A `204` is `submitted` with detail `opencode_prompt_async_accepted`. A timeout, a closed connection, an unreadable response or a `5xx` after the request was written is `submission_unknown`, and the store's claim prevents any replay. Acceptance means the server queued the text for that session; it does not prove the model read it. A reply or an explicit acknowledgment establishes progress, as for the other harnesses.

The notification points to `show` for the exact message ID, as for every harness. A `busy` session can receive it within its current tool loop. On 2026-09-10, OpenCode `1.18.30` read a reply through `htalk wait`, received the native notice before acknowledging it, and completed the current turn without a later extra turn. An idle session starts a turn with whatever model and agent the session already uses. `htalk` passes no model, agent, tools or system prompt.

A separate controlled case delayed preflight until after acknowledgment and the current turn's final response. Before the final acknowledgment check was added, the resulting POST produced a second native model turn for the stale notice. With the check, a new case made no notification POST, stayed idle and retained one final response during the following minute. Each case used one synthetic answer with no replay. These observations cover that ordering, not every possible race.

Acknowledgment after the final check can still race with delivery. The inspected server exposes message-history deletion and session abort, but no pending-notification cancellation operation. htalk does not delete native conversation history or abort a session to hide an acknowledged notice. Cleanup after submission remains unsupported.

## Discovery

`opencode.discover(urls, workspace=None)` is read-only. For each URL it lists sessions (`GET /session`) with their server-reported status (`GET /session/status`); with `workspace` both requests carry `directory=WORKSPACE` so the intended project is queried, otherwise the server's own project answers. Child sessions (`parentID`) and archived sessions are skipped. It then reads the local metadata database at `$XDG_DATA_HOME/opencode/opencode.db` (default `~/.local/share/opencode/opencode.db`, the path printed by `opencode db path`) in read-only mode, taking the 50 most recently updated unarchived root sessions. Saved sessions carry `runtime_status: "unknown"` with `runtime_reason: "saved_metadata_only"` and no URL: a saved session is not evidence of a running server. Titles, messages and credential tables are never read.

Every source entry carries `harness: "opencode"`, a `status` of `ok`, `partial` or `unavailable`, an `error` code and a `detail` code. A malformed or non-loopback URL makes only its own source `unavailable` with `invalid_opencode_url` or `opencode_url_must_be_loopback`, reported without the submitted text; other URLs and the saved metadata are still read. Unreachable, unauthorized, oversized (`opencode_response_too_large`, above 4 MiB) or malformed answers are `unavailable` with a fixed code. When more saved sessions exist than the limit, the saved source is `partial` with `opencode_saved_session_limit_reached`; a missing file is `unavailable` with `opencode_saved_metadata_missing`.

An invalid session record or saved row is skipped individually. Its source becomes `partial` and includes a `rejected` count, while valid addresses before and after it are preserved. If the saved-session limit also applies, its diagnostic remains in `detail`. A malformed response or unreadable database still makes the whole source unavailable.

Limitations: a server lists only the sessions of the directory it serves, so sessions of other projects appear through saved metadata only; a session served by a server on another port is unknown until that URL is supplied; the saved schema is the one observed in `1.18.30` and may change.
