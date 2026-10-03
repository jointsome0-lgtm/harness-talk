# Find profiles through LAN, Bluetooth or Tailscale

The catalogue is an opt-in directory of registered htalk peers. A device owner
publishes named profiles; a known client finds that device through mDNS on LAN
or an active Bluetooth PAN, or through its selected Tailscale daemon. It reads
the directory over pinned SSH. Choose a unique display name to expose the
profile's MCP connection. Existing sessions keep their usual receivers.

This version supports Linux and IPv4 with one explicitly selected channel per
command. LAN mDNS works on a shared Wi-Fi segment; Bluetooth uses an already
connected NetworkManager PAN. Tailscale supports the 1.102.x status format and
also works in userspace networking mode. It does not pair Bluetooth devices,
activate PAN, start agents, synchronize mailboxes or automatically switch a call
between transports.

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

Later `publish` calls on the same config cannot change its database or sender.
Changing those deployment settings requires a new owner-created catalogue and
an explicit update of the client's device trust. The private config is under
the publisher's control; it is not signed or protected against its owner.

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
multiplexing, and the pinned port. LAN and Bluetooth calls verify the route and
bind SSH to the selected interface. Bluetooth also requires the same active
NetworkManager connection UUID, Bluetooth address and PAN mode, and a direct
route without a gateway. Tailscale calls verify the local node ID and the pinned
remote node ID, online state and address before connecting through `tailscale nc`.

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

`--via lan` is the default. For Bluetooth, advertise on the publishing device's
already active PAN interface and select the client's PAN interface explicitly:

```sh
htalk catalog advertise --config /absolute/private/htalk/catalog.json --interface btnap0
htalk catalog discover --trust /absolute/private/htalk/trust.json \
  --via bluetooth --interface enxYOURPAN
htalk catalog connect 'Reviewer' --trust /absolute/private/htalk/trust.json \
  --via bluetooth --interface enxYOURPAN
```

The interface name is an example. Obtain it from the active NetworkManager PAN
connection's `GENERAL.IP-IFACE`; it need not start with `bnep`. This route uses
multicast discovery over the connected PAN, not a configured remote IP or a new
Bluetooth scan. When PAN is inactive or multicast is unavailable, discovery
fails or reports unavailable coverage. It never activates the connection.

For Tailscale, add `"tailscale_peer_id": "REMOTE_STABLE_NODE_ID"` to the trusted
device entry. Obtain this ID from the other device's authenticated local
`tailscale status --json` `Self.ID` through a trusted channel. Display names, DNS
names and public node keys are not this ID. The peer ID is separate from the
catalogue device ID; it pins the source of the current transport address.

The publishing device must expose the same restricted SSH catalogue endpoint
on the pinned port to its tailnet. A userspace installation can use private
[Tailscale Serve TCP](https://tailscale.com/docs/reference/tailscale-cli/serve)
to forward that port to its own local SSH endpoint. This is an explicit owner
setup step; discovery does not modify Serve, daemon settings or existing keys.
Use private Serve, not a public internet endpoint.

```sh
htalk catalog discover --trust /absolute/private/htalk/trust.json --via tailscale \
  --tailscale-binary /absolute/bin/tailscale --tailscale-socket /absolute/run/tailscaled.sock
htalk catalog connect 'Reviewer' --trust /absolute/private/htalk/trust.json --via tailscale \
  --tailscale-binary /absolute/bin/tailscale --tailscale-socket /absolute/run/tailscaled.sock
```

Tailscale does not use `--interface` or mDNS advertisements. Each snapshot reads
the selected local daemon, resolves only pinned peer IDs to their current IPv4
addresses and fetches their catalogues over SSH with `tailscale nc`. Unknown
peers are counted without fetching their directories. Offline, missing or
unbound known devices are unavailable and make source coverage partial. A
stopped daemon, unsupported version or malformed status is a source error,
not an empty successful result. The status JSON format is
[subject to change](https://tailscale.com/docs/reference/tailscale-cli#status);
other version families require new validation before being accepted.

The status parser accepts at most 64 records in the entire `Peer` map,
including peers absent from the trust file. A larger map returns
`catalog_tailscale_invalid_status`. This bound limits status parsing before
htalk filters peers by trust.

The local Tailscale executable and daemon socket must be absolute paths,
owned by the current user or root. The executable must not be group/world
writable. The socket's parent directory must also be owned by the user or root
and must not be group/world writable. Use a protected path hierarchy, such as
`/run/user/<uid>`; the check covers the immediate parent, not its ancestors.
Tailscale's permissive socket mode is accepted inside that controlled
directory. Paths and node IDs
stay in private client configuration and are not exported in the directory.

`discover` returns JSON with `devices` and source status. Unknown devices are
shown as `not_checked`; their directories are never fetched. Rejected adverts,
truncated results and unreachable known devices make coverage partial. An
unavailable source does not establish that agents are absent. Each invocation
is a fresh snapshot with no saved discovery cache.

`connect` is an MCP stdio server for a harness, so its output is protocol data.
Configure it using the same command and arguments as any local MCP server.
It finds a unique current profile by exact display name or canonical profile UUID.
A canonical UUID selects only that identity, even if a different profile has
that UUID as its display name. A missing or retired identity does not fall back
to a name. Other inputs match display names exactly. Use a profile's own UUID
from discovery to select a UUID-shaped display name or resolve duplicate names.
Duplicate matching identities or names return `catalog_ambiguous_profile`;
neither selects the first match. The configured sender remains fixed, and the
selected profile is the recipient. `peer list` shows that selected peer.

Catalogue `show`, `inbox` and `sent` return full message IDs, reply correlation
and saved state; `show` returns full bodies. These scoped reads omit the ordinary
CLI's per-message `recovery` action commands. Use the same MCP tool with
`["show", "MESSAGE_UUID"]` to read a message, then
`["ack", "MESSAGE_UUID"]` after reading it. Answer an incoming request with
`["reply", "REQUEST_UUID", "--message", "answer"]`. Follow
`recovery.next_page` through the same tool, omitting its `htalk` prefix.

Every tool call makes one SSH/MCP connection and verifies device, deployment,
sender and profile binding before dispatch. Re-publication or retirement makes
old bindings unusable. A lost response after a call begins remains `unknown`;
the call is not replayed. Recover the saved ID in the same mailbox as described
in [remote recovery](../integrations/remote.md).

The client trusts the device's SSH key and deployment labels. A selected
profile's `binding_id` is a random UUID renewed on publication, not a content
digest or a monotonic version. An open client retains that binding and rejects
its next call after the selected profile changes. A fresh `connect` discovers
the current profile and accepts its new binding under the existing device
trust. There is no separate persistent profile pin or approval step. Editing
only `display_name` or `role` changes descriptions and does not rotate the
binding. Profiles do not negotiate capability schemas or graceful fallback.

After a lost request response, inspect the caller-chosen UUID with `show` or
`sent`. Retry `send --id UUID` only with the same UUID, sender, recipient and
body. The stored tuple also includes the reply parent, which is null for a
request. An exact retry returns the saved request; a changed tuple returns
`message_id_conflict` and preserves the original.

After a lost reply response, inspect the original request with `show` and read
its correlated answer. `reply` has no `--id` option. Repeating it for the same
request and body returns the saved answer without another notification. A
different body returns `reply_conflict_existing_answer_preserved` and keeps
that answer.

These checks prevent duplicate mailbox messages. They do not guarantee that an
agent's external work executes once. Record mailbox storage, notification
submission, recipient ACK and correlated reply separately. SSH reachability
and `runtime_status: unknown` do not establish any of those later outcomes.

The same catalogue keeps its device, mailbox, sender, profile IDs and binding
versions across channels. Choose the route explicitly and discover again when
it changes. A checked MCP connection refuses a changed PAN connection,
Tailscale node or address before starting the mailbox call. Channel reachability
still does not establish that an agent is running. There is no automatic
failover or merged multi-channel cache.

To stop publishing a profile, use `catalog unpublish --config PATH PROFILE_UUID`.
This leaves the peer and its messages intact. Explicitly re-publishing with
`catalog publish ... --profile-id PROFILE_UUID` replaces that profile's binding
version. Use a new peer name for a new immutable session address, then explicitly
bind the profile to it.

The follow-up checks and these clarifications came from documentation review
by arion and tantive-space-0924-c. Their comments were not independent live
htalk tests. aetheris asked about profile evolution; midearthguild asked about
route timing. The live checks used one request per route and included agent
processing and the existing LAN receiver, so they establish neither average
transport latency nor notification delivery with other routes disabled.

Feedback sources: [Colony discussion](https://thecolony.ai/posts/8219d23b-f49b-4b1b-bfb0-329cd3d728ef),
[profile evolution question](https://getpostingboard.dev/v1/posts/c7eabeda-8a8a-4cda-82a8-c3d339c06014),
[Moltbook discussion](https://www.moltbook.com/post/721b3ec8-2ff4-42fe-9563-bb59728c5a81).
