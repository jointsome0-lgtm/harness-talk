# Reverse tunnel recovery and explicit session resume

Checked on 2026-09-27 with installed htalk 0.9.0, Codex CLI 0.157.1,
GPT-6 Luna at medium effort, Ubuntu 24.04, OpenSSH 9.6p1 and systemd 255.
The mailbox remained on the PC. A temporary PC user service maintained two
reverse Unix-socket forwards to the laptop; a separate receiver targeted one
ordinary Codex TUI. The service used a five-second restart delay.

## Automatic transport recovery

The controller killed only the SSH service's main process and immediately
saved one request. At the send receipt the service had no main PID and was
waiting to restart. systemd then ran `ssh_sockets.py`, removed both refused
socket paths, and restarted SSH without a controller start command. The
service was running again about 5.44 seconds after the interruption.

The original receiver, TUI and client-facing MCP processes survived. The
receiver reconnected and submitted one native notice. The worker read and
ACKed the request, processed 2,048 synthetic rows once, and returned the
result's digest. The PC independently reproduced the expected bytes and ACKed
the correlated answer. The worker also attempted `wait` on the incoming
request; htalk rejected it with `wait_requires_own_request`. No computation or
mailbox write was repeated because of that error.

The helper's separate local regression check preserved an active socket and
ordinary file/symlink paths, then removed a pair of stale sockets. That check
does not establish isolation from another writer using the same OS account.
The [setup](../../integrations/ssh-service.md) requires one supervisor and a
dedicated private directory.

## Resume after a confirmed idle exit

An early controller attempt did not actually close the TUI: `/quit` remained
in its input field, and the controller sent a follow-up after a failed stop
check. That follow-up completed in the original running process. It is an
invalid stopped-worker trial, not evidence of recovery after exit.

After verifying that follow-up and its separate ACK, the controller confirmed
the pending quit command. The TUI exited with status 0; its native process and
MCP child were absent. Only then was a distinct read-only request saved. The
receiver recorded a native queue receipt, while the request remained without
ACK or answer and the stopped session's history stayed unchanged during an
approximately 46.7-second observation.

The controller launched `codex resume` with that exact UUID, workspace, model,
MCP and permission configuration. No new prompt or task text was appended.
The resumed TUI consumed the saved notice, read and ACKed the request, and
returned the existing result's totals. The original execution marker and
result bytes were unchanged. The PC checked the totals and ACKed the answer.
There were three requests, three correlated replies, six separate ACKs and
one computation across the valid and invalid trial segments. Final inboxes
were empty.

## Limits

This checks process interruption and explicit resume after an idle exit.
It does not check physical Wi-Fi loss, suspension, reboot, startup at login,
Bluetooth, WAN, or automatic resumption of interrupted model work. A live
server-side socket causes the helper to refuse cleanup; connection recovery
can therefore wait for the old server-side owner to exit. The receiver's
notification receipts still do not prove task completion.

Only the task's temporary service, receiver, TUI and socket listeners were
used. No SSH daemon, firewall, power setting or login credential was changed.
The installed Rust executable performed all mailbox operations; no local Rust
build was required for this integration check.
