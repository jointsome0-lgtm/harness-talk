//! Resolve one explicit channel; keep identity and mailbox policy in the catalogue.
use super::{capture, code};
use crate::{error::Error, os};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    time::{Duration, Instant},
};

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

#[derive(Clone)]
pub(crate) struct TailClient {
    binary: PathBuf,
    socket: PathBuf,
}

pub(super) struct TailStatus {
    pub local_id: String,
    pub peers: BTreeMap<String, Option<Ipv4Addr>>,
}

impl TailClient {
    pub(super) fn new(binary: &str, socket: &str) -> Result<Self, Error> {
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

    pub(super) fn status(&self) -> Result<TailStatus, Error> {
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

    fn proxy(&self, ip: Ipv4Addr, port: u16) -> String {
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

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Pan {
    connection: String,
    address: String,
    mode: String,
}

impl Pan {
    pub(super) fn read(interface: &str) -> Result<Self, Error> {
        let args = vec![
            "/usr/bin/nmcli".into(),
            "-g".into(),
            "UUID,TYPE".into(),
            "connection".into(),
            "show".into(),
            "--active".into(),
        ];
        let until = Instant::now() + Duration::from_secs(5);
        let bytes = capture(&args, Duration::from_secs(2))?;
        let lines = std::str::from_utf8(&bytes).map_err(|_| code("catalog_pan_unavailable"))?;
        let mut found = Vec::new();
        if lines.lines().count() > 64 {
            return Err(code("catalog_pan_unavailable"));
        }
        for line in lines.lines() {
            let Some((id, "bluetooth")) = line.split_once(':') else {
                continue;
            };
            super::uuid(id)?;
            let budget = until
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(2));
            if budget.is_zero() {
                return Err(code("catalog_pan_unavailable"));
            }
            let args = vec![
                "/usr/bin/nmcli".into(),
                "--escape".into(),
                "no".into(),
                "-t".into(),
                "-f".into(),
                "GENERAL.IP-IFACE,GENERAL.STATE,bluetooth.bdaddr,bluetooth.type".into(),
                "connection".into(),
                "show".into(),
                id.into(),
            ];
            let bytes = capture(&args, budget)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| code("catalog_pan_unavailable"))?;
            let values: BTreeMap<_, _> = text
                .lines()
                .filter_map(|line| line.split_once(':'))
                .collect();
            let mode = values.get("bluetooth.type").copied().unwrap_or_default();
            if values.len() == 4
                && values.get("GENERAL.IP-IFACE") == Some(&interface)
                && values.get("GENERAL.STATE") == Some(&"activated")
                && matches!(mode, "panu" | "nap")
            {
                let address = values["bluetooth.bdaddr"].to_uppercase();
                if address.len() != 17
                    || address.split(':').count() != 6
                    || !address
                        .split(':')
                        .all(|p| p.len() == 2 && p.bytes().all(|c| c.is_ascii_hexdigit()))
                {
                    return Err(code("catalog_pan_unavailable"));
                }
                found.push(Self {
                    connection: id.into(),
                    address,
                    mode: mode.into(),
                });
            }
        }
        if found.len() != 1 {
            return Err(code("catalog_pan_unavailable"));
        }
        Ok(found.pop().unwrap())
    }
}

#[derive(Clone)]
pub(crate) enum Route {
    DirectSsh {
        ip: Ipv4Addr,
    },
    Interface {
        ip: Ipv4Addr,
        interface: String,
        pan: Option<Pan>,
    },
    Tailscale {
        ip: Ipv4Addr,
        client: TailClient,
        local_id: String,
        peer_id: String,
    },
}

impl Route {
    pub(super) fn ip(&self) -> Ipv4Addr {
        match self {
            Self::DirectSsh { ip } | Self::Interface { ip, .. } | Self::Tailscale { ip, .. } => *ip,
        }
    }

    pub(super) fn ssh_args(&self, port: u16) -> Vec<String> {
        match self {
            Self::DirectSsh { .. } => Vec::new(),
            Self::Interface { interface, .. } => vec!["-B".into(), interface.clone()],
            Self::Tailscale { ip, client, .. } => vec!["-o".into(), client.proxy(*ip, port)],
        }
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        match self {
            // The address is captured from owner-controlled trust at discovery.
            // An open client keeps it; no mutable discovery source can redirect it.
            Self::DirectSsh { .. } => {}
            Self::Interface { ip, interface, pan } => {
                let args = vec![
                    "/usr/sbin/ip".into(),
                    "-j".into(),
                    "route".into(),
                    "get".into(),
                    ip.to_string(),
                ];
                let values: Vec<Value> =
                    serde_json::from_slice(&capture(&args, Duration::from_secs(1))?)?;
                if values.len() != 1
                    || values[0]["dev"] != *interface
                    || (pan.is_some() && values[0].get("gateway").is_some())
                {
                    return Err(code("catalog_route_interface_changed"));
                }
                if let Some(expected) = pan
                    && Pan::read(interface)? != *expected
                {
                    return Err(code("catalog_pan_binding_changed"));
                }
            }
            Self::Tailscale {
                ip,
                client,
                local_id,
                peer_id,
            } => {
                let status = client.status()?;
                if status.local_id != *local_id || status.peers.get(peer_id) != Some(&Some(*ip)) {
                    return Err(code("catalog_tailscale_binding_changed"));
                }
            }
        }
        Ok(())
    }
}
