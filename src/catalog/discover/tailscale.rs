//! `--via tailscale`: the peers of one selected Tailscale daemon that the trust file pins.
use super::{Found, Source, fetch};
use crate::{
    catalog::{
        capture, code,
        link::{Link, ssh::Ssh},
        stopped,
        trust::{Trust, accept, peer_id},
    },
    error::Error,
    os,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct Tailscale<'a> {
    pub binary: &'a str,
    pub socket: &'a str,
}
impl Source for Tailscale<'_> {
    fn find(&self, trust: Trust) -> Result<(Value, Vec<Found>), Error> {
        find(self.binary, self.socket, trust)
    }
}

fn find(binary: &str, socket: &str, t: Trust) -> Result<(Value, Vec<Found>), Error> {
    let client = TailClient::new(binary, socket)?;
    let status = client.status()?;
    let unknown = status
        .peers
        .keys()
        .filter(|id| {
            !t.devices
                .iter()
                .any(|d| d.tailscale_peer_id.as_ref() == Some(id))
        })
        .count();
    let end = Instant::now() + Duration::from_secs(15);
    let mut devices = Vec::new();
    let mut found = Vec::new();
    for device in t.devices {
        if stopped() {
            return Err(Error::Interrupted);
        }
        let result = (|| {
            let id = device
                .tailscale_peer_id
                .as_ref()
                .ok_or_else(|| code("catalog_tailscale_not_bound"))?;
            let ip = status
                .peers
                .get(id)
                .and_then(|v| *v)
                .ok_or_else(|| code("catalog_tailscale_peer_unavailable"))?;
            if end.saturating_duration_since(Instant::now()) < Duration::from_secs(1) {
                return Err(code("catalog_discovery_budget_exhausted"));
            }
            let link = Ssh::through_tailscale(
                &device,
                ip,
                client.clone(),
                status.local_id.clone(),
                id.clone(),
            );
            link.check()?;
            let budget = end
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            if budget.is_zero() {
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
                    "via":"tailscale","catalog":item.directory}));
                found.push(item);
            }
            Err(Error::Interrupted) => return Err(Error::Interrupted),
            Err(error) => devices.push(
                json!({"device_id":device.device_id,"trust":"known","directory_state":"unavailable",
                "via":"tailscale","error":error.to_string()}),
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
        json!({"devices":devices,"sources":[{"source":"tailscale","via":"tailscale",
        "status":if partial { "partial" } else { "ok" },"unknown_peer_count":unknown}],
        "scope":"Snapshot from the explicitly selected Tailscale daemon. Only locally pinned peer IDs are fetched. Reachability is not agent liveness."}),
        found,
    ))
}

#[derive(Clone)]
pub(in crate::catalog) struct TailClient {
    binary: PathBuf,
    socket: PathBuf,
}

pub(in crate::catalog) struct TailStatus {
    pub local_id: String,
    pub peers: BTreeMap<String, Option<Ipv4Addr>>,
}

impl TailClient {
    fn new(binary: &str, socket: &str) -> Result<Self, Error> {
        let client = Self {
            binary: binary.into(),
            socket: socket.into(),
        };
        client.validate()?;
        Ok(client)
    }

    fn validate(&self) -> Result<(), Error> {
        for (path, socket) in [(&self.binary, false), (&self.socket, true)] {
            let m = fs::symlink_metadata(path).map_err(|_| {
                code(if socket {
                    "catalog_tailscale_invalid_socket"
                } else {
                    "catalog_tailscale_invalid_binary"
                })
            })?;
            if !path.is_absolute()
                || path.to_str().is_none()
                || !(os::is_roots(&m) || os::is_mine(&m))
                || if socket {
                    !os::is_socket(&m)
                } else {
                    !m.is_file() || os::others_write(&m) || !os::is_executable(&m)
                }
            {
                return Err(code(if socket {
                    "catalog_tailscale_invalid_socket"
                } else {
                    "catalog_tailscale_invalid_binary"
                }));
            }
            if socket {
                // Tailscale intentionally permits connecting to its Unix socket.
                // The parent directory controls who can replace that listener.
                let parent = fs::metadata(path.parent().unwrap())
                    .map_err(|_| code("catalog_tailscale_invalid_socket"))?;
                if !parent.is_dir()
                    || os::others_write(&parent)
                    || !(os::is_roots(&parent) || os::is_mine(&parent))
                {
                    return Err(code("catalog_tailscale_invalid_socket"));
                }
            }
        }
        Ok(())
    }

    pub(in crate::catalog) fn status(&self) -> Result<TailStatus, Error> {
        self.validate()?;
        let args = vec![
            self.binary.to_string_lossy().into_owned(),
            format!("--socket={}", self.socket.display()),
            "status".into(),
            "--json".into(),
        ];
        let bytes = capture(&args, Duration::from_secs(3)).map_err(|e| {
            if matches!(e, Error::Interrupted) {
                e
            } else {
                code("catalog_tailscale_unavailable")
            }
        })?;
        parse_status(&bytes)
    }

    pub(in crate::catalog) fn proxy(&self, ip: Ipv4Addr, port: u16) -> String {
        // OpenSSH expands percent tokens before executing ProxyCommand in a shell.
        fn quote(s: &str) -> String {
            format!("'{}'", s.replace('%', "%%").replace('\'', "'\\''"))
        }
        format!(
            "ProxyCommand={} {} nc {} {}",
            quote(self.binary.to_str().unwrap()),
            quote(&format!("--socket={}", self.socket.display())),
            ip,
            port
        )
    }
}

fn parse_status(bytes: &[u8]) -> Result<TailStatus, Error> {
    let invalid = || code("catalog_tailscale_invalid_status");
    let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if !value["Version"]
        .as_str()
        .is_some_and(|v| v.starts_with("1.102."))
    {
        return Err(code("catalog_tailscale_unsupported_version"));
    }
    if value["BackendState"] != "Running" || value["Self"]["Online"] != true {
        return Err(code("catalog_tailscale_unavailable"));
    }
    let local_id = value["Self"]["ID"]
        .as_str()
        .filter(|v| peer_id(v))
        .ok_or_else(invalid)?;
    let records = value["Peer"].as_object().ok_or_else(invalid)?;
    if records.len() > 64 {
        return Err(invalid());
    }
    let mut peers = BTreeMap::new();
    for record in records.values() {
        let id = record["ID"]
            .as_str()
            .filter(|v| peer_id(v))
            .ok_or_else(invalid)?;
        let online = record["Online"].as_bool().ok_or_else(invalid)?;
        let mut ipv4 = Vec::new();
        for address in record["TailscaleIPs"].as_array().ok_or_else(invalid)? {
            let ip: IpAddr = address
                .as_str()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            if let IpAddr::V4(ip) = ip {
                if ip.octets()[0] != 100 || !(64..=127).contains(&ip.octets()[1]) {
                    return Err(invalid());
                }
                ipv4.push(ip);
            }
        }
        if id == local_id
            || ipv4.len() > 1
            || peers
                .insert(id.into(), online.then(|| ipv4.first().copied()).flatten())
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(TailStatus {
        local_id: local_id.into(),
        peers,
    })
}
