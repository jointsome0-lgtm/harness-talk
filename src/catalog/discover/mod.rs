//! Finding the known devices. Each `--via` value is one `Source`, in a file of its own; all of
//! them give the same `Found` records, each with the `Link` that reaches the device.
mod mdns;
pub(super) mod pan;
mod pinned;
pub(super) mod tailscale;

use super::{
    Browse,
    binding::ProfileBinding,
    capture, code,
    link::Link,
    profile::Directory,
    trust::{Trust, TrustedDevice, trust},
    uuid,
};
use crate::error::Error;
pub(super) use mdns::advertise;
use serde_json::Value;
use std::{path::Path, sync::Arc, time::Duration};

/// One way to find devices. It gives the report `catalog discover` prints and what it reached.
trait Source {
    fn find(&self, trust: Trust) -> Result<(Value, Vec<Found>), Error>;
}

pub(super) struct Found {
    directory: Directory,
    device: TrustedDevice,
    link: Arc<dyn Link>,
}

fn fetch(link: &dyn Link, budget: Duration) -> Result<Directory, Error> {
    Ok(serde_json::from_slice(&capture(
        &link.command("catalog"),
        budget,
    )?)?)
}

pub(super) fn discover(options: &Browse) -> Result<(Value, Vec<Found>), Error> {
    let t = trust(Path::new(&options.trust))?;
    let source: Box<dyn Source + '_> = match options.via.as_str() {
        "ssh" => {
            if options.interface.is_some() {
                return Err(code("catalog_ssh_omit_interface"));
            }
            Box::new(pinned::Pinned)
        }
        "tailscale" => {
            if options.interface.is_some() {
                return Err(code("catalog_tailscale_omit_interface"));
            }
            Box::new(tailscale::Tailscale {
                binary: &options.tailscale_binary,
                socket: &options.tailscale_socket,
            })
        }
        "bluetooth" => Box::new(pan::Bluetooth {
            interface: options.interface(),
            seconds: options.seconds,
        }),
        _ => Box::new(mdns::Lan {
            interface: options.interface(),
            seconds: options.seconds,
        }),
    };
    source.find(t)
}

// Canonical UUID input names an identity and never falls back to a description.
pub(super) fn select_profile(
    found: Vec<Found>,
    selector: &str,
) -> Result<(TrustedDevice, Arc<dyn Link>, ProfileBinding), Error> {
    let identity_only = uuid(selector).is_ok();
    let mut selected = found.into_iter().flat_map(|f| {
        let profiles: Vec<_> = f
            .directory
            .profiles
            .iter()
            .filter(|p| {
                (if identity_only {
                    p.profile_id == selector
                } else {
                    p.display_name == selector
                }) && p.binding_state == "current"
            })
            .map(|p| ProfileBinding {
                profile_id: p.profile_id.clone(),
                binding_id: p.binding_id.clone(),
                peer_name: p.peer_name.clone(),
            })
            .collect();
        profiles
            .into_iter()
            .map(move |p| (f.device.clone(), f.link.clone(), p))
    });
    let (device, link, profile) = selected
        .next()
        .ok_or_else(|| code("catalog_profile_unavailable"))?;
    if selected.next().is_some() {
        return Err(code("catalog_ambiguous_profile"));
    }
    Ok((device, link, profile))
}
