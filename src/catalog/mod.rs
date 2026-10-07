//! Published profiles on known devices. Discovery never registers or messages peers.
//!
//! `profile` is the catalogue file and what it publishes, `binding` what an endpoint is held
//! to, `trust` the list of known devices. `discover` finds those devices, one `Source` for
//! each `--via`, and `link` opens the pipe to a found one. `endpoint` serves.
mod binding;
mod discover;
mod endpoint;
mod link;
mod mailbox;
mod profile;
mod trust;

use crate::{
    error::Error,
    os::{self, Grouped, Open},
    validate,
};
pub(crate) use binding::Binding;
use clap::{Args, Subcommand};
pub(crate) use endpoint::{NO_SCOPE, serve};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const META_KEY: &str = "harness-talk/catalog";
const LIMIT: usize = 262_144;
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn stopped() -> bool {
    STOP.get().is_some_and(|s| s.load(Ordering::Relaxed)) || os::interrupted()
}

fn ssh_port() -> u16 {
    22
}
fn code(s: &str) -> Error {
    Error::code(s)
}
fn uuid(s: &str) -> Result<(), Error> {
    if validate::uuid(s)? != s {
        return Err(code("catalog_requires_canonical_uuid"));
    }
    Ok(())
}
fn text(s: &str, max: usize) -> Result<(), Error> {
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(code("catalog_invalid_text"));
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<File, Error> {
    let f = OpenOptions::new().read(true).no_follow().open(path)?;
    let m = f.metadata()?;
    if !m.is_file() || !os::is_private(&m) || m.len() > LIMIT as u64 {
        return Err(code("catalog_config_must_be_private_owned_file"));
    }
    Ok(f)
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let mut buf = Vec::new();
    private_file(path)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut buf)?;
    if buf.len() > LIMIT {
        return Err(code("catalog_config_too_large"));
    }
    Ok(serde_json::from_slice(&buf)?)
}

/// Runs one tool to its end within `budget` and gives what it wrote.
fn capture(args: &[String], budget: Duration) -> Result<Vec<u8>, Error> {
    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("LC_ALL", "C")
        .own_group()
        .spawn()?;
    let group = child.id() as i32;
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let until = Instant::now() + budget;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
        if Instant::now() >= until || stopped() {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        thread::sleep(Duration::from_millis(25));
    };
    // A ProxyCommand may own descendants that still hold stdout after SSH exits.
    os::kill_group(group);
    let received = reader.join();
    if stopped() {
        return Err(Error::Interrupted);
    }
    let bytes = received.map_err(|_| code("catalog_unreachable"))??;
    if !success || bytes.len() > LIMIT {
        return Err(code("catalog_unreachable"));
    }
    Ok(bytes)
}

const CONFIG: &str = "Private catalogue JSON; each endpoint fixes its own mailbox and sender.";

// The `catalog` commands.
#[derive(Subcommand)]
pub(crate) enum Action {
    #[command(
        about = "Publish an existing peer; replace a profile binding explicitly with --profile-id."
    )]
    Publish(Publish),
    Unpublish {
        #[arg(long, value_name = "config", help = CONFIG)]
        config: String,
        #[arg(value_name = "profile_id")]
        profile_id: String,
    },
    #[command(
        about = "Read only this endpoint's published profiles; no private session addresses."
    )]
    Export {
        #[arg(long, value_name = "config", help = CONFIG)]
        config: String,
    },
    #[command(about = "Fixed SSH endpoint: accepts only the exact remote command catalog or mcp.")]
    Serve {
        #[arg(long, value_name = "config", help = CONFIG)]
        config: String,
    },
    #[command(about = "Advertise this catalogue until stopped; no standalone daemon is installed.")]
    Advertise {
        #[arg(long, value_name = "config", help = CONFIG)]
        config: String,
        #[arg(long, value_name = "interface")]
        interface: String,
        #[arg(
            long,
            value_name = "seconds",
            default_value = "0",
            value_parser = clap::value_parser!(u64).range(0..=86400)
        )]
        seconds: u64,
    },
    #[command(
        about = "Fetch profiles only from known, pinned devices through the selected channel."
    )]
    Discover(Browse),
    #[command(about = "Find this published profile and expose its checked MCP route on stdio.")]
    Connect {
        #[arg(
            value_name = "profile",
            help = "Canonical UUID selects only that profile identity, with no name fallback. Other inputs match display names exactly. For UUID-shaped or duplicate names, use the profile's own UUID."
        )]
        profile: String,
        #[command(flatten)]
        browse: Browse,
    },
}

#[derive(Args)]
pub(crate) struct Publish {
    #[arg(long, value_name = "config", help = CONFIG)]
    config: String,
    #[arg(value_name = "peer")]
    peer: String,
    #[arg(long, value_name = "name")]
    name: String,
    #[arg(long, value_name = "role")]
    role: String,
    #[arg(long, value_name = "device_name", default_value = "htalk device")]
    device_name: String,
    #[arg(long, value_name = "profile_id")]
    profile_id: Option<String>,
    #[arg(
        long,
        value_name = "ssh_port",
        default_value = "22",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    ssh_port: u16,
}

// Where `discover` and `connect` look for known devices.
#[derive(Args)]
pub(crate) struct Browse {
    #[arg(
        long,
        value_name = "trust",
        help = "Private list of known device IDs, mailbox bindings and pinned SSH files."
    )]
    trust: String,
    #[arg(
        long,
        value_name = "interface",
        required_if_eq_any([("via", "lan"), ("via", "bluetooth")]),
        help = "Inspect only this local interface."
    )]
    interface: Option<String>,
    #[arg(
        long,
        value_name = "via",
        default_value = "lan",
        value_parser = ["lan", "bluetooth", "tailscale", "ssh"],
        help = "Choose one channel; never fails over a mailbox call."
    )]
    via: String,
    #[arg(
        long,
        value_name = "tailscale_binary",
        default_value = "/usr/bin/tailscale",
        help = "Owned Tailscale 1.102.x executable, used only with --via tailscale."
    )]
    tailscale_binary: String,
    #[arg(
        long,
        value_name = "tailscale_socket",
        default_value = "/var/run/tailscale/tailscaled.sock",
        help = "Local Tailscale daemon socket, used only with --via tailscale."
    )]
    tailscale_socket: String,
    #[arg(
        long,
        value_name = "seconds",
        default_value = "5",
        value_parser = clap::value_parser!(u64).range(1..=30)
    )]
    seconds: u64,
}

impl Browse {
    /// Read by the lan and bluetooth channels. The parser asks for it only when --via is
    /// spelled out, so the default channel without one still stops here, as before.
    fn interface(&self) -> &str {
        self.interface.as_deref().unwrap()
    }
}

fn emit(v: &Value) -> Result<(), Error> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, v)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

/// Runs a catalogue command and gives its exit code.
pub(crate) fn main(action: &Action, db: Option<&str>, actor: Option<&str>) -> i32 {
    match run(action, db, actor) {
        Ok(()) => 0,
        Err(error) => {
            if matches!(action, Action::Connect { .. } | Action::Serve { .. }) {
                eprintln!("htalk catalog: {error}");
            } else {
                let _ = emit(&json!({"state":"error","error":error.to_string()}));
            }
            if matches!(error, Error::Interrupted) {
                130
            } else {
                2
            }
        }
    }
}

fn run(action: &Action, db: Option<&str>, actor: Option<&str>) -> Result<(), Error> {
    let signals = if matches!(
        action,
        Action::Advertise { .. } | Action::Discover(_) | Action::Connect { .. }
    ) {
        let flag = STOP
            .get_or_init(|| Arc::new(AtomicBool::new(false)))
            .clone();
        Some(os::Stop::on(flag)?)
    } else {
        None
    };
    if !matches!(action, Action::Publish(_)) && (db.is_some() || actor.is_some()) {
        return Err(code("catalog_binding_fixed_omit_db_and_as"));
    }
    match action {
        Action::Publish(options) => emit(&profile::publish(options, db, actor)?),
        Action::Unpublish { config, profile_id } => {
            emit(&profile::unpublish(Path::new(config), profile_id)?)
        }
        Action::Export { config } => emit(&serde_json::to_value(profile::directory(Path::new(
            config,
        ))?)?),
        Action::Serve { config } => endpoint::ssh(Path::new(config)),
        Action::Advertise {
            config,
            interface,
            seconds,
        } => discover::advertise(config, interface, *seconds),
        Action::Discover(browse) => emit(&discover::discover(browse)?.0),
        Action::Connect { profile, browse } => {
            text(profile, 128)?;
            let (_, found) = discover::discover(browse)?;
            let (device, link, profile) = discover::select_profile(found, profile)?;
            let expected = Binding {
                schema_version: 1,
                device_id: device.device_id.clone(),
                mailbox_id: device.mailbox_id.clone(),
                generation: device.generation.clone(),
                sender: device.sender.clone(),
                profiles: vec![profile],
            };
            // The persistent MCP server owns signals after discovery finishes.
            drop(signals);
            endpoint::checked(link, expected).map_err(|_| code("catalog_mcp_failed"))
        }
    }
}
