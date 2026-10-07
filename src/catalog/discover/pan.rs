//! `--via bluetooth`: the same advertisements as on a LAN, read on the interface of one active
//! Bluetooth network that NetworkManager reports.
use super::{Found, Source, mdns};
use crate::{
    catalog::{capture, code, trust::Trust, uuid},
    error::Error,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(super) struct Bluetooth<'a> {
    pub interface: &'a str,
    pub seconds: u64,
}
impl Source for Bluetooth<'_> {
    fn find(&self, trust: Trust) -> Result<(Value, Vec<Found>), Error> {
        let pan = Pan::read(self.interface)?;
        mdns::browse(self.interface, self.seconds, "bluetooth", trust, Some(pan))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::catalog) struct Pan {
    connection: String,
    address: String,
    mode: String,
}

impl Pan {
    pub(in crate::catalog) fn read(interface: &str) -> Result<Self, Error> {
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
            uuid(id)?;
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
