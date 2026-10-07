//! `--via ssh`: the addresses pinned in the trust file, and nothing else.
use super::{Found, Source, fetch};
use crate::{
    catalog::{
        code,
        link::ssh::Ssh,
        stopped,
        trust::{Trust, accept, ssh_address},
    },
    error::Error,
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct Pinned;
impl Source for Pinned {
    fn find(&self, trust: Trust) -> Result<(Value, Vec<Found>), Error> {
        find(trust)
    }
}

fn find(t: Trust) -> Result<(Value, Vec<Found>), Error> {
    let end = Instant::now() + Duration::from_secs(15);
    let mut devices = Vec::new();
    let mut found = Vec::new();
    for device in t.devices {
        if stopped() {
            return Err(Error::Interrupted);
        }
        let result = (|| {
            let address = device
                .ssh_address
                .as_ref()
                .ok_or_else(|| code("catalog_ssh_not_bound"))?;
            let link = Ssh::pinned(&device, ssh_address(address)?);
            let budget = end
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            if budget < Duration::from_secs(1) {
                return Err(code("catalog_discovery_budget_exhausted"));
            }
            let directory = fetch(&link, budget)?;
            accept(&device, &directory)?;
            Ok(Found {
                directory,
                device: device.clone(),
                link: Arc::new(link),
            })
        })();
        match result {
            Ok(item) => {
                devices.push(json!({"device_id":device.device_id,"trust":"known","directory_state":"reachable",
                    "via":"ssh","catalog":item.directory}));
                found.push(item);
            }
            Err(Error::Interrupted) => return Err(Error::Interrupted),
            Err(error) => devices.push(
                json!({"device_id":device.device_id,"trust":"known","directory_state":"unavailable",
                "via":"ssh","error":error.to_string()}),
            ),
        }
    }
    if stopped() {
        return Err(Error::Interrupted);
    }
    let partial = devices
        .iter()
        .any(|d| d["directory_state"] == "unavailable");
    Ok((
        json!({"devices":devices,"sources":[{"source":"ssh","via":"ssh",
        "status":if partial { "partial" } else { "ok" }}],
        "scope":"Snapshot from explicitly pinned SSH IPv4 addresses. No multicast or Tailscale discovery. Reachability is not agent liveness."}),
        found,
    ))
}
