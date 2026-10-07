//! `--via lan`: devices that advertise the service on one local interface, and the advertiser.
//! An advertisement only selects a pinned device. It never establishes trust.
use super::{Found, Source, fetch, pan::Pan};
use crate::{
    catalog::{
        code, emit,
        link::{Link, ssh::Ssh},
        profile::{current, load},
        stopped,
        trust::{Trust, accept},
        uuid,
    },
    error::Error,
    os,
};
use mdns_sd::{
    DaemonEvent, DaemonStatus, IfKind, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo,
    UnregisterStatus,
};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const SERVICE: &str = "_htalk._tcp.local.";

pub(super) struct Lan<'a> {
    pub interface: &'a str,
    pub seconds: u64,
}
impl Source for Lan<'_> {
    fn find(&self, trust: Trust) -> Result<(Value, Vec<Found>), Error> {
        browse(self.interface, self.seconds, "lan", trust, None)
    }
}

struct Daemon {
    daemon: ServiceDaemon,
    closed: Cell<bool>,
}
impl std::ops::Deref for Daemon {
    type Target = ServiceDaemon;
    fn deref(&self) -> &ServiceDaemon {
        &self.daemon
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.closed.get() {
            return;
        }
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }
}

fn shutdown(d: &Daemon) -> Result<(), Error> {
    let done = d
        .shutdown()
        .map_err(|_| code("catalog_mdns_shutdown_unconfirmed"))?;
    if done
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| code("catalog_mdns_shutdown_unconfirmed"))?
        != DaemonStatus::Shutdown
    {
        return Err(code("catalog_mdns_shutdown_unconfirmed"));
    }
    d.closed.set(true);
    Ok(())
}

fn daemon(interface: &str) -> Result<Daemon, Error> {
    if interface.is_empty()
        || interface.len() > 32
        || !interface
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
        || !Path::new("/sys/class/net").join(interface).exists()
    {
        return Err(code("catalog_interface_not_found"));
    }
    let flags = fs::read_to_string(Path::new("/sys/class/net").join(interface).join("flags"))?;
    let flags = u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
        .map_err(|_| code("catalog_interface_unavailable"))?;
    if !os::multicast_ready(flags) {
        return Err(code("catalog_interface_unavailable"));
    }
    let d = Daemon {
        daemon: ServiceDaemon::new().map_err(|_| code("catalog_mdns_unavailable"))?,
        closed: Cell::new(false),
    };
    d.disable_interface(IfKind::All)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    d.enable_interface(interface)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    Ok(d)
}

pub(super) fn browse(
    interface: &str,
    seconds: u64,
    via: &str,
    t: Trust,
    pan: Option<Pan>,
) -> Result<(Value, Vec<Found>), Error> {
    let d = daemon(interface)?;
    let monitor = d.monitor().map_err(|_| code("catalog_mdns_unavailable"))?;
    let events = d
        .browse(SERVICE)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut services = BTreeMap::new();
    let mut truncated = false;
    let mut disconnected = false;
    while Instant::now() < end && !stopped() {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(100)) {
            match event {
                ServiceEvent::ServiceResolved(s) => {
                    if services.len() < 64 || services.contains_key(s.get_fullname()) {
                        services.insert(s.get_fullname().to_owned(), s);
                    } else {
                        truncated = true;
                    }
                }
                ServiceEvent::ServiceRemoved(_, name) => {
                    services.remove(&name);
                }
                _ => {}
            }
        } else if events.is_disconnected() {
            disconnected = true;
            break;
        }
    }
    let failed = disconnected
        || monitor
            .try_iter()
            .any(|e| matches!(e, DaemonEvent::Error(_)));
    let _ = d.stop_browse(SERVICE);
    shutdown(&d)?;
    if stopped() {
        return Err(Error::Interrupted);
    }
    if failed {
        return Err(code("catalog_mdns_unavailable"));
    }
    let mut devices = Vec::new();
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut rejected = 0;
    let fetch_end = Instant::now() + Duration::from_secs(15);
    for service in services.into_values() {
        if stopped() {
            return Err(Error::Interrupted);
        }
        // TXT is decoded by mdns-sd. It selects a known pin, never establishes trust.
        let Some(id) = service.get_property_val_str("device") else {
            rejected += 1;
            continue;
        };
        if uuid(id).is_err()
            || service.get_property_val_str("v") != Some("1")
            || service.get_port() == 0
        {
            rejected += 1;
            continue;
        }
        if seen.contains(id) {
            continue;
        }
        let Some(device) = t.devices.iter().find(|d| d.device_id == id) else {
            seen.insert(id.to_owned());
            devices.push(json!({"device_id":id,"trust":"unknown","directory_state":"not_checked"}));
            continue;
        };
        if service.get_port() != device.ssh_port {
            devices.push(json!({"device_id":id,"trust":"known","directory_state":"unavailable","error":"catalog_ssh_port_mismatch"}));
            continue;
        }
        let mut addresses: Vec<_> = service
            .get_addresses()
            .iter()
            .filter_map(|ip| match ip {
                ScopedIp::V4(v) if v.interface_ids().iter().any(|i| i.name == interface) => {
                    Some(*v.addr())
                }
                _ => None,
            })
            .filter(|ip| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_loopback())
            .collect();
        addresses.sort();
        let result = addresses
            .first()
            .ok_or_else(|| code("catalog_ipv4_unavailable"))
            .and_then(|ip| {
                if fetch_end.saturating_duration_since(Instant::now()) < Duration::from_secs(1) {
                    return Err(code("catalog_discovery_budget_exhausted"));
                }
                let link = Ssh::on_interface(device, *ip, interface, pan.clone());
                link.check()?;
                let budget = fetch_end
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(5));
                if budget.is_zero() {
                    return Err(code("catalog_discovery_budget_exhausted"));
                }
                let directory = fetch(&link, budget)?;
                accept(device, &directory)?;
                Ok(Found {
                    directory,
                    device: device.clone(),
                    link: Arc::new(link),
                })
            });
        match result {
            Ok(item) => { seen.insert(id.to_owned()); devices.push(json!({"device_id":id,"trust":"known","directory_state":"reachable","catalog":item.directory})); found.push(item); },
            Err(error) => devices.push(json!({"device_id":id,"trust":"known","directory_state":"unavailable","error":error.to_string()})),
        }
    }
    if stopped() {
        return Err(Error::Interrupted);
    }
    let partial = devices
        .iter()
        .any(|d| d["directory_state"] == "unavailable")
        || rejected > 0
        || truncated;
    Ok((
        json!({"devices":devices,"sources":[{"source":"mdns","interface":interface,"via":via,
        "status":if partial { "partial" } else { "ok" },"rejected":rejected,"truncated":truncated}],
        "scope":"Snapshot of advertised devices on the selected interface. Unavailable sources do not prove absence; channel reachability is not agent liveness."}),
        found,
    ))
}

pub(in crate::catalog) fn advertise(
    config: &str,
    interface: &str,
    seconds: u64,
) -> Result<(), Error> {
    let path = Path::new(config);
    let c = load(path)?;
    current(&c)?;
    let d = daemon(interface)?;
    let monitor = d.monitor().map_err(|_| code("catalog_mdns_unavailable"))?;
    let props = [("v", "1"), ("device", c.device_id.as_str())];
    let mut info = ServiceInfo::new(
        SERVICE,
        &format!("htalk-{}", c.device_id),
        &format!("htalk-{}.local.", c.device_id),
        "",
        c.ssh_port,
        &props[..],
    )
    .map_err(|_| code("catalog_invalid_advertisement"))?
    .enable_addr_auto();
    info.set_interfaces(vec![IfKind::Name(interface.into())]);
    let fullname = info.get_fullname().to_owned();
    d.register(info)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    let ready_end = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < ready_end && !stopped() {
        match monitor.recv_timeout(Duration::from_millis(100)) {
            Ok(DaemonEvent::Announce(name, _)) if name == fullname => {
                ready = true;
                break;
            }
            Ok(DaemonEvent::Error(_)) => break,
            _ => {}
        }
    }
    if !ready {
        shutdown(&d)?;
        if stopped() {
            return Err(Error::Interrupted);
        }
        return Err(code("catalog_mdns_announcement_unconfirmed"));
    }
    emit(&json!({"state":"advertising","device_id":c.device_id,"interface":interface}))?;
    let end = (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds));
    let mut result = Ok(());
    while !stopped() && end.is_none_or(|end| Instant::now() < end) {
        if monitor
            .try_iter()
            .any(|e| matches!(e, DaemonEvent::Error(_)))
        {
            result = Err(code("catalog_mdns_unavailable"));
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let removed = d
        .unregister(&fullname)
        .map_err(|_| code("catalog_mdns_unregister_unconfirmed"))
        .and_then(|done| {
            done.recv_timeout(Duration::from_secs(2))
                .map_err(|_| code("catalog_mdns_unregister_unconfirmed"))
        })
        .and_then(|status| {
            if matches!(status, UnregisterStatus::OK) {
                Ok(())
            } else {
                Err(code("catalog_mdns_unregister_unconfirmed"))
            }
        });
    let closed = shutdown(&d);
    result.and(removed).and(closed)
}
