//! The OpenSSH link: a pinned key and host key for one device, over the path discovery found.
use super::Link;
use crate::{
    catalog::{
        capture, code,
        discover::{pan::Pan, tailscale::TailClient},
        trust::TrustedDevice,
    },
    error::Error,
    os,
};
use serde_json::Value;
use std::{net::Ipv4Addr, time::Duration};

pub(in crate::catalog) struct Ssh {
    device: TrustedDevice,
    route: Route,
}

impl Ssh {
    /// To an address pinned in the trust file.
    pub(in crate::catalog) fn pinned(device: &TrustedDevice, ip: Ipv4Addr) -> Self {
        Self::new(device, Route::DirectSsh { ip })
    }

    /// Out of one local interface, which may be a Bluetooth network.
    pub(in crate::catalog) fn on_interface(
        device: &TrustedDevice,
        ip: Ipv4Addr,
        interface: &str,
        pan: Option<Pan>,
    ) -> Self {
        let interface = interface.into();
        Self::new(device, Route::Interface { ip, interface, pan })
    }

    /// Through the selected Tailscale daemon, to one of its peers.
    pub(in crate::catalog) fn through_tailscale(
        device: &TrustedDevice,
        ip: Ipv4Addr,
        client: TailClient,
        local_id: String,
        peer_id: String,
    ) -> Self {
        let route = Route::Tailscale {
            ip,
            client,
            local_id,
            peer_id,
        };
        Self::new(device, route)
    }

    fn new(device: &TrustedDevice, route: Route) -> Self {
        Self {
            device: device.clone(),
            route,
        }
    }
}

impl Link for Ssh {
    fn command(&self, operation: &str) -> Vec<String> {
        let (d, route) = (&self.device, &self.route);
        let mut args = vec![
            os::SSH.into(),
            "-F".into(),
            "/dev/null".into(),
            "-T".into(),
            "-p".into(),
            d.ssh_port.to_string(),
            "-i".into(),
            d.identity_file.to_string_lossy().into_owned(),
            "-l".into(),
            d.ssh_user.clone(),
        ];
        args.extend(route.ssh_args(d.ssh_port));
        for option in [
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=/dev/null",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "ControlMaster=no",
            "ControlPath=none",
            "ControlPersist=no",
            "ConnectTimeout=3",
            "ConnectionAttempts=1",
            "ServerAliveInterval=2",
            "ServerAliveCountMax=1",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        args.extend([
            "-o".into(),
            format!("UserKnownHostsFile={}", d.known_hosts_file.display()),
            "-o".into(),
            format!("HostKeyAlias=htalk-{}", d.device_id),
            route.ip().to_string(),
            operation.into(),
        ]);
        args
    }

    fn check(&self) -> Result<(), Error> {
        self.route.check()
    }
}

// How the link builds its command line and checks that the path has not changed.
enum Route {
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
    fn ip(&self) -> Ipv4Addr {
        match self {
            Self::DirectSsh { ip } | Self::Interface { ip, .. } | Self::Tailscale { ip, .. } => *ip,
        }
    }

    fn ssh_args(&self, port: u16) -> Vec<String> {
        match self {
            Self::DirectSsh { .. } => Vec::new(),
            Self::Interface { interface, .. } => vec!["-B".into(), interface.clone()],
            Self::Tailscale { ip, client, .. } => vec!["-o".into(), client.proxy(*ip, port)],
        }
    }

    fn check(&self) -> Result<(), Error> {
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
