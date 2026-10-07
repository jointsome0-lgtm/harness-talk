//! `peer`: find sessions and keep the registry of their addresses. None of these messages a peer.
use super::{PeerAdd, PeerCommand};
use crate::{
    cli::{Answer, Context},
    error::Error,
    model::{Harness, Peer},
    notify, validate,
};
use serde_json::json;

pub(crate) fn run(command: &PeerCommand, context: &mut Context) -> Answer {
    let value = match command {
        PeerCommand::Discover {
            harness,
            workspace,
            codex_socket,
            opencode_url,
        } => {
            return discover(
                harness.as_deref(),
                workspace.as_deref(),
                codex_socket.as_deref(),
                opencode_url.as_deref(),
            );
        }
        PeerCommand::Add(add) => {
            // The address is checked before the mailbox is opened or created.
            let registration = registration(add)?;
            let peer = context.open(true)?.register(&registration)?;
            let mut value = serde_json::to_value(&peer)?;
            if peer.retired_at.is_some() {
                let name = &add.name;
                value["recovery"] =
                    json!({"restore":context.recovery(&["peer", "restore", name], false)});
                value["next_action"] = format!("Peer {name} is retired, and peer add does not change that. If this session should get new requests again, use recovery.restore.").into();
            }
            value
        }
        PeerCommand::Check { name } => {
            let peer = context.open(false)?.peer(name)?;
            let mut value = notify::probe(&peer)?;
            value["retired_at"] = json!(peer.retired_at);
            value
        }
        PeerCommand::Retire { name } => serde_json::to_value(context.open(false)?.retire(name)?)?,
        PeerCommand::Restore { name } => serde_json::to_value(context.open(false)?.restore(name)?)?,
        PeerCommand::List { all } => {
            let peers = context.open(false)?.peers()?;
            let total = peers.len();
            let visible: Vec<_> = peers
                .into_iter()
                .filter(|p| *all || p.retired_at.is_none())
                .collect();
            let hidden = total - visible.len();
            let mut value = json!({"peers": visible});
            if !all {
                value["retired_hidden"] = hidden.into();
            }
            value
        }
    };
    Ok((value, 0))
}

fn discover(
    harness: Option<&str>,
    workspace: Option<&str>,
    codex: Option<&[String]>,
    opencode: Option<&[String]>,
) -> Answer {
    let harness = harness.map(str::parse::<Harness>).transpose()?;
    let value = crate::discovery::discover(harness, workspace, codex, opencode)?;
    let failed = value["sources"]
        .as_array()
        .is_none_or(|sources| sources.iter().all(|s| s["status"] == "unavailable"));
    Ok((value, if failed { 2 } else { 0 }))
}

pub(crate) fn registration(add: &PeerAdd) -> Result<Peer, Error> {
    if add.delivery == "pull" {
        if add.session.is_some()
            || add.workspace.is_some()
            || add.socket.is_some()
            || add.url.is_some()
        {
            return Err(Error::code("pull_peer_has_no_native_address"));
        }
        validate::peer_name(&add.name)?;
        validate::peer_name(&add.harness).map_err(|_| Error::code("invalid_harness_id"))?;
        return Ok(Peer::pull(&add.name, &add.harness));
    }
    // The parser requires these two for native delivery.
    notify::native_peer(
        &add.name,
        add.harness.parse()?,
        add.session.as_deref().unwrap_or_default(),
        add.workspace.as_deref().unwrap_or_default(),
        add.socket.as_deref(),
        add.url.as_deref(),
    )
}
