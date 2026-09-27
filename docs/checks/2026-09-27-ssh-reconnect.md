# Mail on the PC while a worker is disconnected

On 2026-09-27, one ordinary Codex CLI 0.157.1 session on a Linux laptop handled
two requests saved on a separate Linux PC while its mailbox connection was
unavailable. The controller verified both answers independently. The same TUI
and local MCP process survived the failed connection and both incoming turns.

This checks a development build of the 0.9.0 `mcp --connect` and `receive`
commands against an installed 0.8.1 mailbox endpoint. It is separate from the
[0.8.1 colocated native check](2026-09-27-ssh-native.md). The mailbox used schema
3; this work adds no storage migration.

## Topology and task

The PC held the only mailbox, with two fresh pull peers. Fixed MCP and watch
commands accepted connections through private Unix sockets. The PC initiated
a pinned, authenticated reverse SSH tunnel to private sockets on the laptop.
No PC SSH server, public TCP listener, firewall change or new credential was
needed. The owner controlled the tunnel; the worker used its sockets.

The laptop ran one ordinary Luna medium TUI with normal native permission
checks. Its local MCP command used `mcp --connect`. A separate `receive`
process bound the remote watch peer to that exact saved CLI session and
workspace. Neither process selected a different model or changed permissions.
The synthetic input and compute script were copied before launch. The script
created an exclusive execution receipt so a second invocation would fail.

## Observed sequence

1. With no tunnel, the bootstrap called `inbox` once. The tool reported a
   connection failure before sending a mailbox command; its local MCP process
   stayed alive. The TUI finished that turn.
2. The PC saved a UUID-addressed computation request while disconnected.
   For the next observed 35.2 seconds, the worker's history remained unchanged
   and no computation result existed.
3. The controller connected the tunnel. The receiver recorded one native queue
   receipt. That notice started the same TUI, which read and ACKed the request,
   processed 6,000 synthetic CSV rows and replied with the result's hash.
   The PC independently reproduced all group counts, dot products and sums of
   squares, then compared the serialized result byte for byte and ACKed the reply.
4. A fixture run exposed premature watch disconnection because the receiver
   closed its connector's stdin. The implementation was corrected to hold it
   open. Only the idle receiver was restarted, preserving its binding and
   receipt. The TUI and MCP process stayed running. The corrected connection
   remained open and its idle history did not grow.
5. The controller stopped only the tunnel and saved a second request. During
   a 105.9-second offline observation, the worker's history and computation
   receipt remained unchanged.
6. The first tunnel restart failed because server-side Unix socket files
   remained. After confirming both old tunnel processes had exited and each
   owned socket refused connections, the controller removed those two stale
   files and restored the same route. It did not resend the request.
7. The receiver delivered the second notice to the same TUI. The worker read
   the saved result and returned dot sum `-2741243` and squares sum `623149675`,
   without rerunning the compute script. The PC verified and ACKed the answer.

The final mailbox contained two requests and their two correlated replies,
all acknowledged. Both inboxes were empty. The native history recorded one
failed bootstrap tool call, six successful mailbox calls, two incoming notices
and exactly one `python3 compute.py` execution. The computation receipt and
result bytes stayed unchanged through the second outage.

The controller stopped its receiver, TUI, MCP, tmux, SSH tunnel and socket
listeners, then retired both test peers. The owner's original SSH access and
installed htalk remained unchanged. Synthetic files and receipts were retained.

## Failure coverage and limits

Automated checks also cover a write committed before its response is lost,
connection failure followed by success in the same MCP client, cancellation
and process cleanup through the remote connection, watch replay after restart,
a truncated watch frame and refusal to restart an uncertain native submission.
They use local fixtures and do not establish WAN reliability.

The transport interruption was controlled SSH disconnection, not a physical
Wi-Fi outage, power loss or sleep. The controller restored the reverse tunnel;
htalk reconnected its tool calls and watch stream once that route was available.
This is not unattended SSH tunnel management or worker startup. The model was
cloud-hosted; the computation ran on the laptop.

Queue receipts and ACKs do not prove completed work. The result comparison
supplied that evidence here. Arbitrary side effects are not exactly once.
Other remote harness receivers, Bluetooth, native macOS/Windows and a shared
execution-status protocol remain unverified.
