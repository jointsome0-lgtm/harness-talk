# Find profiles on a LAN

The catalogue is an opt-in directory of registered htalk peers. A device owner
publishes named profiles; a known client finds that device through mDNS and
reads its directory over pinned SSH. Choose a unique display name to expose the
profile's MCP connection. Existing sessions keep their usual receivers.

This first version supports Linux, IPv4 and one explicitly selected LAN
interface. It works on a shared Wi-Fi segment. It does not discover across
routers, start agents, synchronize mailboxes or switch between transports.
Tailscale and Bluetooth discovery are not included.

## Publish existing peers

Choose a private catalogue file in an owner-controlled directory. Register the
participants in the usual way first. The endpoint's `--as` is the sender used
for all incoming mailbox calls. Publish the peers that sender may address.

```sh
mkdir -p /absolute/private/htalk
chmod 700 /absolute/private/htalk
htalk --db /absolute/mail.sqlite3 --as visitor catalog publish \
  --config /absolute/private/htalk/catalog.json reviewer \
  --name 'Reviewer' --role 'Review requested changes' --device-name 'Workstation'
htalk --db /absolute/mail.sqlite3 --as visitor catalog publish \
  --config /absolute/private/htalk/catalog.json builder \
  --name 'Builder' --role 'Implement agreed changes'
htalk catalog export --config /absolute/private/htalk/catalog.json
```

The first publication fixes the local database, sender, device ID and deployment
labels. `--ssh-port PORT` can set a nondefault SSH port on that first publication;
the default is 22. Directory reads require schema 3 and never create or migrate
a database. Export contains only published profiles, their role, harness,
delivery mode and binding state. It omits native session addresses, workspace
paths and history.

`runtime_status` is `unknown`. A saved profile, a pull peer or a reachable SSH
endpoint does not prove that an agent is running. `binding_state` is `current`,
`retired` or `changed`; only current profiles can be selected.

## Configure a restricted SSH endpoint

Use a dedicated client key and a server-owned forced command. Its authorized
key entry has this form; replace paths and the public key with real values:

```text
restrict,command="/absolute/bin/htalk catalog serve --config /absolute/private/htalk/catalog.json" ssh-ed25519 PUBLIC_KEY
```

`catalog serve` accepts only the exact SSH original command `catalog` or `mcp`.
The former exports the directory. The latter starts MCP with the catalogue's
fixed database and sender. No shell command is taken from an advertisement.
Use the SSH server's normal restrictions for the allowed OS account and
network. Keep unrelated keys and endpoints separate.

The dedicated SSH principal can access all profiles published in this
catalogue. Server-side checks hide other peers and reject their conversations
for send, show, reply, ACK and wait. Inbox/sent counts and pagination are computed
after the published-peer filter. A selected profile further narrows each normal
catalogue-proxy call; it is a selector, not a new authorization grant.

## Introduce the device once

Create a mode-600 trust file on the client. Obtain IDs and the sender from a
trusted copy of `catalog export`. Copy and verify the SSH host public key
through an already trusted channel. Do not use an unauthenticated LAN advert to
establish trust.

```json
{
  "schema_version": 1,
  "devices": [{
    "device_id": "5ba7cdd8-07aa-4eec-8bb4-2c5c46d12b2b",
    "mailbox_id": "f642936e-c5fd-41b4-93bd-6ae89a3b43f8",
    "generation": "257008bd-3b57-4450-965d-7008a7c901c0",
    "sender": "visitor",
    "ssh_user": "alice",
    "ssh_port": 22,
    "identity_file": "/absolute/private/htalk/client-key",
    "known_hosts_file": "/absolute/private/htalk/known_hosts"
  }]
}
```

Its separate known-hosts file uses the device's alias rather than a changing IP:

```text
htalk-5ba7cdd8-07aa-4eec-8bb4-2c5c46d12b2b ssh-ed25519 VERIFIED_HOST_PUBLIC_KEY
```

Private keys and trust files stay outside the repository. SSH uses strict
host-key checking, the configured key without an agent, no forwarding or
multiplexing, and the pinned port. Both discovery and each later mailbox call
verify that the selected interface routes to the device; SSH binds its source
to that interface.

The mailbox ID and generation are owner-pinned deployment labels in the
catalogue sidecar, not UUIDs stored inside SQLite. The catalogue also checks the
database's device/inode and full sender/peer records. Replacing the file or
changing a peer invalidates its binding. An owner rewriting the database in
place with identical peer records must recreate the catalogue and trust
binding explicitly. Do not treat a matching path or label as proof that two
independent databases contain the same history.

## Find and choose

On the publishing device, leave this command running while profiles should be
visible. It does not install a background service. Ctrl-C or SIGTERM removes the
advertisement and closes the mDNS daemon. `--seconds N` bounds a trial.

```sh
htalk catalog advertise --config /absolute/private/htalk/catalog.json \
  --interface wlan0
```

On the known client:

```sh
htalk catalog discover --trust /absolute/private/htalk/trust.json --interface wlan0
htalk catalog connect 'Reviewer' --trust /absolute/private/htalk/trust.json --interface wlan0
```

`discover` returns JSON with `devices` and source status. Unknown devices are
shown as `not_checked`; their directories are never fetched. Rejected adverts,
truncated results and unreachable known devices make coverage partial. An
unavailable source does not establish that agents are absent. Each invocation
is a fresh snapshot with no saved discovery cache.

`connect` is an MCP stdio server for a harness, so its output is protocol data.
Configure it using the same command and arguments as any local MCP server.
It finds a unique current profile by exact display name or profile UUID.
Duplicate names require the UUID from discovery; names never silently select
the first match. The configured sender remains fixed, and the selected profile
is the recipient. `peer list` shows that selected peer.

Every tool call makes one SSH/MCP connection and verifies device, deployment,
sender and profile binding before dispatch. Re-publication or retirement makes
old bindings unusable. A lost response after a call begins remains `unknown`;
the call is not replayed. Recover the saved ID in the same mailbox as described
in [remote recovery](../integrations/remote.md).

To stop publishing a profile, use `catalog unpublish --config PATH PROFILE_UUID`.
This leaves the peer and its messages intact. Explicitly re-publishing with
`catalog publish ... --profile-id PROFILE_UUID` replaces that profile's binding
version. Use a new peer name for a new immutable session address, then explicitly
bind the profile to it.
