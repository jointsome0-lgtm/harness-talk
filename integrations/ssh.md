# One mailbox across two devices

An MCP client can run the existing `htalk mcp` command through SSH. The mailbox
stays on one device; SSH carries the stdio protocol. This uses htalk 0.8.1 as
shipped, with no new daemon, database schema or transport implementation.

Choose a mailbox host that will remain available. It can be either device.
The [Linux LAN pilot](../docs/checks/2026-09-27-ssh-pilot.md) put it on the worker
laptop because that machine already accepted SSH. Its mailbox becomes
unavailable when the laptop sleeps. Putting the mailbox on an always-on PC
allows requests to remain queued while a worker is disconnected.

## Prepare the mailbox host

Use an existing, owner-authorized SSH account and an installed `htalk` binary.
Start with a separate mailbox. The following paths and addresses are examples;
replace them with the actual account, installation and network addresses.

```sh
umask 077
mkdir -p /home/mailbox/.local/share/harness-talk/lan
htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 \
  peer add remote-agent --harness generic --delivery pull
htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 \
  peer add worker --harness generic --delivery pull
```

Configure the worker's local [MCP entry](mcp.md) with this database and
`--as worker`. Register only the intended participants in this mailbox.
Use a separate name and SSH key for each remote participant.

The `worker` above is a pull peer. To notify an already-running Codex TUI,
use the [native worker setup](#notify-an-already-running-codex-tui) below with
a fresh peer name instead of registering that pull worker.

## Bind the remote key to one participant

Generate a dedicated SSH key on the connecting device. Keep its private half
there; add only its public half to the mailbox account's `authorized_keys`.
Preserve existing keys. Prefix the new public key with these options, on one
line, using the connecting device's actual source address:

```text
restrict,from="192.0.2.10",command="/home/mailbox/.local/bin/htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 --as remote-agent mcp" ssh-ed25519 PUBLIC_KEY_HERE htalk-remote-agent
```

The absolute command fixes the executable, database and peer. It does not use
`SSH_ORIGINAL_COMMAND` or client-provided shell text. OpenSSH's `restrict`
option disables forwarding, PTYs and `~/.ssh/rc`; `command=`
replaces the requested command. See the [OpenSSH key-options reference](https://man.openbsd.org/sshd.8#AUTHORIZED_KEYS_FILE_FORMAT).
Keep `.ssh` private and `authorized_keys` writable only by its owner. A changed
source address must be handled explicitly; do not remove restrictions to make
an unexplained connection failure disappear.

Verify the server's host-key fingerprint through a trusted channel before
adding it to the client's dedicated known-hosts file. A key obtained only with
`ssh-keyscan` has not yet been authenticated. Keep strict checking enabled.

## Connect the remote agent's MCP client

Use the client's MCP configuration format. This common JSON form launches SSH
without a terminal or a remote command; the key supplies the fixed command:

```json
{
  "mcpServers": {
    "htalk": {
      "command": "ssh",
      "args": [
        "-F", "/dev/null", "-T",
        "-i", "/home/agent/.ssh/htalk-device",
        "-o", "IdentitiesOnly=yes",
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=yes",
        "-o", "UserKnownHostsFile=/home/agent/.ssh/htalk-known-hosts",
        "-o", "GlobalKnownHostsFile=/dev/null",
        "-o", "ForwardAgent=no",
        "-o", "ForwardX11=no",
        "-o", "ClearAllForwardings=yes",
        "-o", "ConnectTimeout=10",
        "-o", "ConnectionAttempts=1",
        "-o", "ServerAliveInterval=5",
        "-o", "ServerAliveCountMax=2",
        "mailbox@192.0.2.20"
      ]
    }
  }
}
```

Use a client-side SSH agent for a passphrase-protected key; unlock it before
starting the MCP client. `BatchMode` makes missing authentication fail instead
of waiting for an interactive password prompt. Do not forward that agent.

The tool is still `htalk`, with `{"args":["inbox"]}` and the same
[send/read/ACK/reply commands](mcp.md#use-the-tool). Global identity/database
overrides, registration and file input are unavailable through MCP. Peer
listing and message permissions follow the ordinary mailbox rules. The route
does not provide a per-recipient allowlist within that mailbox, resource quotas
or isolation from its trusted OS account; use a separate mailbox for a separate
collaboration. Do not expose an owner's general mailbox to untrusted users.

## Notify an already-running Codex TUI

When the mailbox and the worker's ordinary Codex TUI are on the same host,
the existing Codex adapter can deliver a native notice after an SSH client
saves a message. No remote app-server listener or new receiver is needed.
[Two successive incoming turns](../docs/checks/2026-09-27-ssh-native.md) were
verified with htalk 0.8.1 and Codex CLI 0.157.1 on Linux.

Start the ordinary TUI in its intended workspace, with the local
[MCP entry](mcp.md) fixed to this mailbox and `--as codex-worker`. Keep its
normal model and permission settings. Register that fresh native name before
using mailbox tools; the MCP server can start before registration.
Give the receiving session its owner-authorized task scope before sending work;
a peer notification grants no additional permissions.

In a separate terminal under the same mailbox account and Codex environment,
find the exact session UUID and confirm its workspace:

```sh
htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 \
  peer discover --harness codex
htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 \
  peer add codex-worker --harness codex --session EXACT_SESSION_UUID \
  --workspace /home/mailbox/work/project
htalk --db /home/mailbox/.local/share/harness-talk/lan/mail.sqlite3 \
  peer check codex-worker
```

Choose the ordinary CLI session, not another session with a similar name or
the same working directory. Native approval helpers can share its directory.
Use a fresh peer name when the address changes; registration does not convert
an existing pull peer or reassign an existing native peer.

The remote agent now sends to `codex-worker` through its existing restricted
SSH MCP connection. On the mailbox host, htalk calls the local `codex queue`
with the registered UUID. It supplies no model or permission overrides. Make
sure `codex` is on the PATH seen by the forced SSH command, and that this
environment resolves the same Codex metadata as the receiving TUI.

An ordinary native `peer check` verifies saved identity and reports
`runtime_status: unknown`. It does not prove a running client or queue
consumption. Confirm a real read, ACK and correlated reply from the worker.
A saved request or a successful queue receipt alone is insufficient.

The TUI must remain running and the device awake. This route does not launch
a stopped client or wake a sleeping computer. For the first connection,
observe the worker finish one turn, become idle, and then handle a separate
incoming request without Enter, resume or another client launch. The checked
route adds no recurring model polling; the client still controls its own
turns and background work.

## Work and recovery

For the pull setup, start the worker's authorized model turn separately;
pull delivery saves the request but does not wake an idle agent. The native
Codex setup above can notify its running TUI. Give the worker a bounded task
and verify the output independently; an ACK proves neither execution nor success.
Computation runs where the worker executes its tools. Its model can still be
hosted by a cloud provider.

Save a UUID and the exact body before `send`. After an SSH disconnect, reconnect
and use `show` or `sent` to discover the saved outcome. If a retry is needed,
keep the same UUID and body. A matching replay returns the original message
with `created: false`; a different body under that UUID is rejected. Never
turn a transport retry into a new task launch. htalk's saved-message identity
does not make arbitrary worker side effects exactly once.

Keep SQLite on its mailbox host. Transfer task artifacts separately, recording
their sizes and digests. A path in a message refers to its originating device;
it is not automatically accessible on the other one.

To stop access, close active MCP/SSH connections and remove only the dedicated
key's line from `authorized_keys`; revocation does not terminate existing
connections. Retire finished test peers and preserve their receipts. Bluetooth,
WAN operation and startup of stopped workers remain unverified. Native idle
reception was checked only for the Codex TUI described above.
