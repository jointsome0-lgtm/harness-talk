# Local receiver ownership and obsolete watch events

Checked on 2026-09-27 with Linux x86-64, Python 3.12 and Rust 1.96.0.
The baseline used installed htalk 0.9.1. The ownership regression used a
source-built 0.9.2 candidate. All mailboxes, native metadata and Codex queue
executables in these checks were synthetic; no model or real conversation
was used.

## One owner for a saved local session

On 0.9.1, two concurrent receivers with the same Codex session and message ID,
but different `--state` directories, each called the native queue and saved a
receipt. A second process using the same state directory was rejected.

The candidate adds a lock for the resolved Codex SQLite store and session.
The regression check holds a fake native queue call open, then attempts a
second receiver with another state directory. It also changes `CODEX_HOME`
while directing `sqlite_home` to the original store. The second receiver exits
with code 2 before another native call. A different saved session in that
store can still run concurrently.

After SIGTERM, the first receiver reaps its watch connector while the native
queue call is still blocked. A competing receiver remains rejected during
that interval. Releasing the fake queue lets shutdown complete; the saved
uncertain submission still prevents an automatic restart. Existing reconnect
and receipt-deduplication checks also pass.

This is one added CLI regression method. Cargo formatting, clippy with denied
warnings, all 49 Rust tests and all 59 Python CLI/integration tests passed.
No dependency or mailbox-schema change was made.

The lock coordinates cooperating processes on the local filesystem. It does
not cover older receivers that ignore it, independently copied Codex stores,
network filesystem semantics or a same-account process removing the lock.
After shutdown, keep the original receipt directory; ownership does not copy
receipts into another directory or make uncertain external work retryable.

## A buffered notification can become obsolete

A separate baseline fixture captured a real `watch` frame for a request, ACKed
the request, then saved its reply. The inbox contained one request after ACK
and zero after the reply. Delivering the earlier frame to the receiver still
produced one native queue attempt.

This confirms an unnecessary-wakeup window, not duplicate execution: the
fixture had no model or external task. The current receiver has no second
mailbox read before submission. A future readback must retain ACKed requests
that have no reply and must preserve the authenticated remote endpoint.
Local `skip_reason` and the inbox predicate are not interchangeable. A
check-then-act read would reduce obsolete notices without making external
effects exactly once.

## Shutdown observation

Two early sandbox runs timed out after SIGTERM during teardown. A subsequent
comparison of the same installed 0.9.1 executable and dual-receiver fixture
exited normally on the ordinary host, including closed output pipes. The
sandbox comparison still timed out; attaching a tracer there was denied.
The exact restricted signal/IPC mechanism was not established.

The timeout is therefore not evidence that ordinary host shutdown is broken.
No signal-handler change was made. Both the ownership cancellation regression
and the original receiver tests ran with normal host signal/IPC access. All
fixture processes were stopped; no Rust compiler was installed on the main
machine.
