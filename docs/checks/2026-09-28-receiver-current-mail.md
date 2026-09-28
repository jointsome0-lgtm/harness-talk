# Current mail before a native wakeup

Checked on 2026-09-28 UTC with the htalk `0.9.3` x86-64 publication wheel,
commit `e98701dbb2c01194254341603f5f8290afddceee`, SHA-256
`49cf45b55ec31d8bc17852827cbf74d1098e731e8b94f5f977c215cd098829bc`.
The [publication run](https://github.com/jointsome0-lgtm/harness-talk/actions/runs/36360798674) passed both Linux architecture builds and
installed-wheel checks. All 96 included Git files matched the release
tree. Published PyPI files and a fresh installation matched the checked bytes.

The publication wheel used a real fixed MCP endpoint and an owned, stopped
Codex release-check session. A captured watch stream contained an answered
request, an ACKed answer, and an ACKed request without a reply. The receiver
skipped the first two and saved exactly one native submission receipt for the
third. It did not ACK or reply for the worker.

To test the check/start race, an executable shim changed only a private Codex
alias config to an empty decoy store immediately before executing the installed
Codex CLI. The receiver's explicit `sqlite_home` override kept the original
store. The real CLI returned a queue receipt, the decoy database was not created,
and native RPC read back and deleted that exact queue entry. No thread was
resumed and no extra model turn was started by this check.

The CLI regression suite also checks an unavailable or mismatched mailbox,
checker cancellation, preserved legacy receipts, live store selection changes,
and a file symlink targeting a differently named database. With a fake native
CLI, the symlink case exposed a `sqlite_home` value selecting a sibling
`state_5.sqlite`; the fixed code refuses that path before invoking Codex. Two test methods were
added, taking the suite to 49 Rust and 61 CLI tests.

The receiver needs an explicit `--mcp-command` for the same authenticated mailbox
and peer as its watch endpoint. Old watch-only commands retain their previous
behavior. A successful read can become stale before work starts; this is not
exactly-once task execution. Physical Wi-Fi loss, sleep, Bluetooth, internet
deployment and native Windows/macOS were outside this check.
