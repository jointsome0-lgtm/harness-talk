//! The trust file: the devices this account knows, each pinned to a mailbox binding and to
//! SSH files. Nothing a device says about itself adds to it.
use super::{code, profile::Directory, read, text, uuid};
use crate::{error::Error, os, validate};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrustedDevice {
    pub(super) device_id: String,
    pub(super) mailbox_id: String,
    pub(super) generation: String,
    pub(super) sender: String,
    pub(super) ssh_user: String,
    #[serde(default = "super::ssh_port")]
    pub(super) ssh_port: u16,
    pub(super) identity_file: PathBuf,
    pub(super) known_hosts_file: PathBuf,
    #[serde(default)]
    pub(super) tailscale_peer_id: Option<String>,
    #[serde(default)]
    pub(super) ssh_address: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Trust {
    schema_version: u8,
    pub(super) devices: Vec<TrustedDevice>,
}

pub(super) fn peer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

pub(super) fn ssh_address(address: &str) -> Result<Ipv4Addr, Error> {
    let ip: Ipv4Addr = address
        .parse()
        .map_err(|_| code("catalog_invalid_ssh_address"))?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err(code("catalog_invalid_ssh_address"));
    }
    Ok(ip)
}

pub(super) fn trust(path: &Path) -> Result<Trust, Error> {
    let t: Trust = read(path)?;
    if t.schema_version != 1 || t.devices.len() > 16 {
        return Err(code("catalog_invalid_trust"));
    }
    let mut seen = HashSet::new();
    let mut tail_ids = HashSet::new();
    for d in &t.devices {
        for id in [&d.device_id, &d.mailbox_id, &d.generation] {
            uuid(id)?;
        }
        validate::peer_name(&d.sender)?;
        if let Some(id) = &d.tailscale_peer_id
            && (!peer_id(id) || !tail_ids.insert(id))
        {
            return Err(code("catalog_invalid_trust"));
        }
        if let Some(address) = &d.ssh_address {
            ssh_address(address)?;
        }
        if !seen.insert(&d.device_id)
            || d.ssh_port == 0
            || d.ssh_user.is_empty()
            || d.ssh_user.len() > 64
            || !d
                .ssh_user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(code("catalog_invalid_trust"));
        }
        for path in [&d.identity_file, &d.known_hosts_file] {
            let m = fs::symlink_metadata(path)?;
            if !path.is_absolute() || !m.is_file() || !os::is_mine(&m) || os::others_write(&m) {
                return Err(code("catalog_invalid_ssh_file"));
            }
        }
    }
    Ok(t)
}

pub(super) fn accept(d: &TrustedDevice, directory: &Directory) -> Result<(), Error> {
    if directory.schema_version != 1
        || directory.device_id != d.device_id
        || directory.mailbox_id != d.mailbox_id
        || directory.generation != d.generation
        || directory.sender != d.sender
        || directory.profiles.len() > 64
    {
        return Err(code("catalog_endpoint_binding_mismatch"));
    }
    text(&directory.device_name, 128)?;
    let mut seen = HashSet::new();
    for p in &directory.profiles {
        uuid(&p.profile_id)?;
        uuid(&p.binding_id)?;
        validate::peer_name(&p.peer_name)?;
        text(&p.display_name, 128)?;
        text(&p.role, 1024)?;
        validate::peer_name(&p.harness)?;
        if !seen.insert(&p.profile_id)
            || !["native", "pull"].contains(&p.delivery.as_str())
            || !["current", "retired", "changed"].contains(&p.binding_state.as_str())
            || p.runtime_status != "unknown"
        {
            return Err(code("catalog_invalid_profile"));
        }
    }
    Ok(())
}
