# Keep a reverse SSH route available

For a mailbox PC without an SSH server, a Linux user service can maintain the
two private reverse forwards described in [remote reception](remote.md).
This manages transport only. It neither starts a model nor resubmits mailbox
calls or worker tasks. Use htalk 0.9.0 or newer and Python 3.11+ for the small
socket-preparation helper, included in the 0.9.1 source archive or this checkout.
Mailbox schema 3 is unchanged. No Rust build is needed.

## Prepare the endpoints

Keep the PC's fixed MCP and watch socket listeners running. Each must invoke
only the intended mailbox and worker identity. Copy `ssh_sockets.py` from this
checkout to an owner-controlled location on the laptop. Create a dedicated
directory with mode 0700 for `mcp.sock`, `watch.sock` and the helper's lock file.
Do not share it with another bridge or place source files in it.

The connecting SSH account must be allowed to run this fixed preparation
command and forward Unix sockets. A key restricted to `htalk mcp` cannot run
the helper. Preserve host-key pinning, explicit key selection, disabled agent
and X11 forwarding, `BatchMode=yes`, a finite `ConnectTimeout`, and
`ServerAliveInterval`/`ServerAliveCountMax` in an owner-prepared SSH config.
The example below calls that host alias `htalk-laptop`.

The helper removes only these two socket files, and only when their directory
is private, the account owns them, and both refuse connections. It refuses
active endpoints, symlinks, ordinary files and uncertain results. Probing can
open and immediately close an empty endpoint connection; it sends no MCP call.
The helper is not a tunnel lock. Run one supervisor for this directory and let
its previous SSH process exit before preparation.

## Start a user service

Replace every example path. Save the unit as
`~/.config/systemd/user/htalk-laptop.service` on the PC:

```ini
[Unit]
Description=htalk private route to the laptop
StartLimitIntervalSec=0

[Service]
Type=exec
UMask=0077
ExecStartPre=/usr/bin/ssh -F /home/controller/.config/htalk/laptop.ssh htalk-laptop /usr/bin/python3 -B /home/worker/.local/share/htalk/ssh_sockets.py /home/worker/.local/share/htalk/endpoints
ExecStart=/usr/bin/ssh -F /home/controller/.config/htalk/laptop.ssh -N -T -o ExitOnForwardFailure=yes -R /home/worker/.local/share/htalk/endpoints/mcp.sock:/home/controller/.local/share/htalk/lan/mcp.sock -R /home/worker/.local/share/htalk/endpoints/watch.sock:/home/controller/.local/share/htalk/lan/watch.sock htalk-laptop
Restart=always
RestartSec=10
TimeoutStartSec=30
TimeoutStopSec=10

[Install]
WantedBy=default.target
```

If the chosen key uses an SSH agent, make its socket available to this user
service through the owner's usual session setup. Do not copy credentials to
the worker or put a password in the unit. Check the prepared configuration
before starting it:

```sh
systemd-analyze --user verify ~/.config/systemd/user/htalk-laptop.service
systemctl --user daemon-reload
systemctl --user start htalk-laptop.service
systemctl --user status htalk-laptop.service
journalctl --user -u htalk-laptop.service -n 30
```

Starting is separate from enabling at login. Choose `enable` only when this
route should start with that user session. Neither operation wakes a sleeping
computer. The mailbox stays on the PC while the laptop is unavailable.

After SSH exits, systemd runs preparation again and starts a fresh connection.
The client-side `StreamLocalBindUnlink` option does not clean server-side socket
paths. `ExitOnForwardFailure` detects a failed bind, not a failed destination:
check a real MCP read and a correlated worker answer separately. If an old
server-side listener still accepts connections, preparation waits by failing
without unlinking it. Inspect prolonged failures instead of replacing it.

Stop explicitly with `systemctl --user stop htalk-laptop.service`; an explicit
stop does not trigger automatic restart. Preserve the receiver's state and
saved mail. Stop the receiver separately when retiring the worker.

## Keep worker recovery separate

The receiver remains bound to one exact Codex UUID and workspace. Its queue
receipt is not evidence of a running TUI or finished work. Do not put a model
command under this transport service's `Restart=always` policy.

Before resuming a stopped worker, inspect its last turn, saved request/reply
and receiver state. A cleanly idle session can be explicitly resumed with its
exact UUID and the same MCP, workspace, model and permission settings. Do not
use `--last`, create a replacement peer, or append the old task as a new prompt.
An interrupted task or pending unknown notification needs its own recovery;
SSH reconnection does not make repeating its side effects safe.

References: [SSH forward failures](https://man.openbsd.org/ssh_config.5#ExitOnForwardFailure),
[client socket setting](https://man.openbsd.org/ssh_config.5#StreamLocalBindUnlink),
[server socket setting](https://man.openbsd.org/sshd_config.5#StreamLocalBindUnlink),
[systemd service lifecycle](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html).
