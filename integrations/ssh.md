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

## Work and recovery

Start the worker's authorized model turn separately. Pull delivery saves the
request but does not wake an idle agent. Give the worker a bounded task and
verify the output independently; an ACK proves neither execution nor success.
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
WAN operation and automatic worker wakeups were not tested in this pilot.
