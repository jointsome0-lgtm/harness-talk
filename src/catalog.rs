//! Published profiles on known devices. Discovery never registers or messages peers.
use crate::{error::Error, model::Peer, os, store, validate};
use clap::{Arg, ArgMatches, Command as Cli};
use mdns_sd::{
    DaemonEvent, DaemonStatus, IfKind, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo,
    UnregisterStatus,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::Ipv4Addr,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SERVICE: &str = "_htalk._tcp.local.";
pub(crate) const META_KEY: &str = "harness-talk/catalog";
const LIMIT: usize = 262_144;
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn stopped() -> bool {
    STOP.get().is_some_and(|s| s.load(Ordering::Relaxed)) || os::interrupted()
}

struct Mdns {
    daemon: ServiceDaemon,
    closed: Cell<bool>,
}
impl std::ops::Deref for Mdns {
    type Target = ServiceDaemon;
    fn deref(&self) -> &ServiceDaemon {
        &self.daemon
    }
}
impl Drop for Mdns {
    fn drop(&mut self) {
        if self.closed.get() {
            return;
        }
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }
}

struct Signals(Vec<signal_hook::SigId>);
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        if let Some(flag) = STOP.get() {
            flag.store(false, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DbStamp {
    device: u64,
    inode: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Published {
    profile_id: String,
    display_name: String,
    role: String,
    binding_id: String,
    peer: Peer,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u8,
    device_id: String,
    device_name: String,
    #[serde(default = "ssh_port")]
    ssh_port: u16,
    mailbox_id: String,
    generation: String,
    database: PathBuf,
    database_stamp: DbStamp,
    sender: Peer,
    profiles: Vec<Published>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileBinding {
    profile_id: String,
    binding_id: String,
    peer_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    schema_version: u8,
    device_id: String,
    mailbox_id: String,
    generation: String,
    sender: String,
    profiles: Vec<ProfileBinding>,
}

impl Binding {
    fn validate(&self) -> Result<(), Error> {
        if self.schema_version != 1 || self.profiles.len() > 64 {
            return Err(code("catalog_invalid_binding"));
        }
        for id in [&self.device_id, &self.mailbox_id, &self.generation] {
            uuid(id)?;
        }
        validate::peer_name(&self.sender)?;
        let mut seen = HashSet::new();
        for p in &self.profiles {
            uuid(&p.profile_id)?;
            uuid(&p.binding_id)?;
            validate::peer_name(&p.peer_name)?;
            if !seen.insert(&p.profile_id) {
                return Err(code("catalog_duplicate_profile"));
            }
        }
        Ok(())
    }

    pub(crate) fn profile_names(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.peer_name.clone()).collect()
    }

    pub(crate) fn command_scope(
        &self,
        db: &Path,
        name: &str,
        options: &ArgMatches,
    ) -> Result<Option<Value>, Error> {
        let allowed = |name: &str| self.profiles.iter().any(|p| p.peer_name == name);
        match name {
            "send" => {
                if !allowed(required(options, "recipient")) {
                    return Err(code("catalog_profile_not_published"));
                }
            }
            "show" | "ack" | "reply" | "wait" => {
                let m = store::catalog_message(db, required(options, "message_id"), &self.sender)
                    .map_err(|_| code("catalog_conversation_unavailable"))?;
                let other = if m.row.sender == self.sender {
                    &m.row.recipient
                } else {
                    &m.row.sender
                };
                if !allowed(other) {
                    return Err(code("catalog_conversation_unavailable"));
                }
                if name == "show" {
                    return Ok(Some(serde_json::to_value(m)?));
                }
            }
            "peer" => {
                let (sub, o) = options.subcommand().unwrap();
                if sub == "check" && !allowed(required(o, "name")) {
                    return Err(code("catalog_profile_not_published"));
                }
                if sub == "list" {
                    return Ok(Some(
                        json!({"peers":self.profiles.iter().map(|p| json!({"name":p.peer_name,"profile_id":p.profile_id,"binding_id":p.binding_id,"runtime_status":"unknown"})).collect::<Vec<_>>(),"retired_hidden":0}),
                    ));
                }
            }
            "inbox" | "sent" => {
                let sent = name == "sent";
                let names: Vec<_> = self.profiles.iter().map(|p| p.peer_name.clone()).collect();
                let limit = required(options, "limit")
                    .parse::<i64>()
                    .map_err(|_| code("limit_must_be_between_1_and_500"))?;
                let cursor = options
                    .get_one::<String>(if sent { "before_seq" } else { "after_seq" })
                    .map(|s| {
                        s.parse::<i64>()
                            .map_err(|_| code("seq_cursor_must_be_a_positive_integer"))
                    })
                    .transpose()?;
                let page = store::catalog_page(
                    db,
                    &self.sender,
                    &names,
                    sent,
                    limit,
                    cursor,
                    sent && options.get_flag("bodies"),
                )?;
                let next = page.messages.last().filter(|_| page.omitted > 0).map(|m| {
                    format!(
                        "htalk {} --{} {}",
                        name,
                        if sent { "before-seq" } else { "after-seq" },
                        m["seq"]
                    )
                });
                let mut value = serde_json::to_value(page)?;
                if let Some(next) = next {
                    value["recovery"] = json!({"next_page":next});
                }
                return Ok(Some(value));
            }
            _ => {}
        }
        Ok(None)
    }

    /// Every checked connection must reach this device, mailbox, sender and profile binding.
    pub(crate) fn matches(&self, value: Option<&Value>) -> bool {
        let Some(actual) = value.and_then(|v| serde_json::from_value::<Self>(v.clone()).ok())
        else {
            return false;
        };
        actual.validate().is_ok()
            && self.schema_version == actual.schema_version
            && self.device_id == actual.device_id
            && self.mailbox_id == actual.mailbox_id
            && self.generation == actual.generation
            && self.sender == actual.sender
            && self.profiles.iter().all(|p| actual.profiles.contains(p))
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedDevice {
    device_id: String,
    mailbox_id: String,
    generation: String,
    sender: String,
    ssh_user: String,
    #[serde(default = "ssh_port")]
    ssh_port: u16,
    identity_file: PathBuf,
    known_hosts_file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trust {
    schema_version: u8,
    devices: Vec<TrustedDevice>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    profile_id: String,
    display_name: String,
    role: String,
    binding_id: String,
    peer_name: String,
    harness: String,
    delivery: String,
    binding_state: String,
    runtime_status: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Directory {
    schema_version: u8,
    device_id: String,
    device_name: String,
    mailbox_id: String,
    generation: String,
    sender: String,
    profiles: Vec<Profile>,
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
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o077 != 0
        || m.len() > LIMIT as u64
    {
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

fn stamp(path: &Path) -> Result<DbStamp, Error> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.uid() != unsafe { libc::getuid() } {
        return Err(code("catalog_mailbox_not_owned_file"));
    }
    Ok(DbStamp {
        device: m.dev(),
        inode: m.ino(),
    })
}

fn load(path: &Path) -> Result<Catalog, Error> {
    let c: Catalog = read(path)?;
    if c.schema_version != 1
        || c.ssh_port == 0
        || c.profiles.len() > 64
        || !c.database.is_absolute()
    {
        return Err(code("catalog_invalid_config"));
    }
    uuid(&c.device_id)?;
    uuid(&c.mailbox_id)?;
    uuid(&c.generation)?;
    text(&c.device_name, 128)?;
    if stamp(&c.database)? != c.database_stamp {
        return Err(code("catalog_mailbox_replaced"));
    }
    validate::peer_name(&c.sender.name)?;
    let mut seen = HashSet::new();
    for p in &c.profiles {
        uuid(&p.profile_id)?;
        uuid(&p.binding_id)?;
        text(&p.display_name, 128)?;
        text(&p.role, 1024)?;
        validate::peer_name(&p.peer.name)?;
        if !seen.insert(&p.profile_id) {
            return Err(code("catalog_duplicate_profile"));
        }
    }
    Ok(c)
}

fn current(c: &Catalog) -> Result<Vec<Peer>, Error> {
    let mut names = vec![c.sender.name.clone()];
    names.extend(c.profiles.iter().map(|p| p.peer.name.clone()));
    let rows = store::catalog_peers(&c.database, &names)?;
    if rows[0] != c.sender || rows[0].retired_at.is_some() {
        return Err(code("catalog_sender_binding_changed"));
    }
    Ok(rows.into_iter().skip(1).collect())
}

fn directory(path: &Path) -> Result<Directory, Error> {
    let c = load(path)?;
    let rows = current(&c)?;
    let profiles = c
        .profiles
        .iter()
        .zip(rows)
        .map(|(p, row)| Profile {
            profile_id: p.profile_id.clone(),
            display_name: p.display_name.clone(),
            role: p.role.clone(),
            binding_id: p.binding_id.clone(),
            peer_name: p.peer.name.clone(),
            harness: p.peer.harness.clone(),
            delivery: p.peer.delivery.as_str().into(),
            binding_state: if row.retired_at.is_some() {
                "retired"
            } else if row != p.peer {
                "changed"
            } else {
                "current"
            }
            .into(),
            runtime_status: "unknown".into(),
        })
        .collect();
    Ok(Directory {
        schema_version: 1,
        device_id: c.device_id,
        device_name: c.device_name,
        mailbox_id: c.mailbox_id,
        generation: c.generation,
        sender: c.sender.name,
        profiles,
    })
}

pub(crate) fn local_binding(path: &Path, db: &Path, sender: &str) -> Result<Binding, Error> {
    let c = load(path)?;
    if c.database != os::resolve(db) || c.sender.name != sender {
        return Err(code("catalog_endpoint_binding_mismatch"));
    }
    let d = directory(path)?;
    Ok(Binding {
        schema_version: 1,
        device_id: d.device_id,
        mailbox_id: d.mailbox_id,
        generation: d.generation,
        sender: d.sender,
        profiles: d
            .profiles
            .into_iter()
            .filter(|p| p.binding_state == "current")
            .map(|p| ProfileBinding {
                profile_id: p.profile_id,
                binding_id: p.binding_id,
                peer_name: p.peer_name,
            })
            .collect(),
    })
}

pub(crate) fn read_binding(path: &Path) -> Result<Binding, Error> {
    let b: Binding = read(path)?;
    b.validate()?;
    if b.profiles.is_empty() {
        return Err(code("catalog_invalid_binding"));
    }
    Ok(b)
}

fn lock(path: &Path) -> Result<File, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| code("catalog_config_requires_directory"))?;
    fs::create_dir_all(parent)?;
    let m = fs::metadata(parent)?;
    if m.uid() != unsafe { libc::getuid() } || m.mode() & 0o022 != 0 {
        return Err(code("catalog_directory_not_owned"));
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_extension("lock"))?;
    let m = f.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::getuid() } || m.mode() & 0o077 != 0 {
        return Err(code("catalog_invalid_lock"));
    }
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(code("catalog_writer_active"));
    }
    Ok(f)
}

fn save(path: &Path, c: &Catalog) -> Result<(), Error> {
    let bytes = serde_json::to_vec_pretty(c)?;
    if bytes.len() > LIMIT {
        return Err(code("catalog_config_too_large"));
    }
    let temp = path
        .parent()
        .unwrap()
        .join(format!(".catalog-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn publish(options: &ArgMatches, db: Option<&str>, actor: Option<&str>) -> Result<Value, Error> {
    let path = PathBuf::from(required(options, "config"));
    let _lock = lock(&path)?;
    let name = required(options, "peer");
    validate::peer_name(name)?;
    let mut c = if path.exists() {
        load(&path)?
    } else {
        let database = os::resolve(Path::new(
            db.ok_or_else(|| code("catalog_publish_requires_db_and_as"))?,
        ));
        let sender = actor.ok_or_else(|| code("catalog_publish_requires_db_and_as"))?;
        let rows = store::catalog_peers(&database, &[sender.into()])?;
        if rows[0].retired_at.is_some() {
            return Err(code("peer_retired"));
        }
        Catalog {
            schema_version: 1,
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: required(options, "device_name").into(),
            ssh_port: *options.get_one::<u16>("ssh_port").unwrap(),
            mailbox_id: uuid::Uuid::new_v4().to_string(),
            generation: uuid::Uuid::new_v4().to_string(),
            database_stamp: stamp(&database)?,
            database,
            sender: rows[0].clone(),
            profiles: vec![],
        }
    };
    if db.is_some_and(|db| os::resolve(Path::new(db)) != c.database)
        || actor.is_some_and(|a| a != c.sender.name)
    {
        return Err(code("catalog_endpoint_binding_mismatch"));
    }
    let row = store::catalog_peers(&c.database, &[name.into()])?.remove(0);
    if row.retired_at.is_some() {
        return Err(code("peer_retired"));
    }
    let id = options
        .get_one::<String>("profile_id")
        .cloned()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid(&id)?;
    let display_name = required(options, "name").to_owned();
    let role = required(options, "role").to_owned();
    text(&display_name, 128)?;
    text(&role, 1024)?;
    text(&c.device_name, 128)?;
    c.profiles.retain(|p| p.profile_id != id);
    if c.profiles.len() >= 64 {
        return Err(code("catalog_too_many_profiles"));
    }
    c.profiles.push(Published {
        profile_id: id.clone(),
        display_name,
        role,
        binding_id: uuid::Uuid::new_v4().to_string(),
        peer: row,
    });
    current(&c)?;
    save(&path, &c)?;
    Ok(
        json!({"state":"published", "profile_id":id, "device_id":c.device_id,
        "mailbox_id":c.mailbox_id, "generation":c.generation, "sender":c.sender.name}),
    )
}

fn trust(path: &Path) -> Result<Trust, Error> {
    let t: Trust = read(path)?;
    if t.schema_version != 1 || t.devices.len() > 16 {
        return Err(code("catalog_invalid_trust"));
    }
    let mut seen = HashSet::new();
    for d in &t.devices {
        for id in [&d.device_id, &d.mailbox_id, &d.generation] {
            uuid(id)?;
        }
        validate::peer_name(&d.sender)?;
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
            if !path.is_absolute()
                || !m.is_file()
                || m.uid() != unsafe { libc::getuid() }
                || m.mode() & 0o022 != 0
            {
                return Err(code("catalog_invalid_ssh_file"));
            }
        }
    }
    Ok(t)
}

fn connector(d: &TrustedDevice, ip: Ipv4Addr, interface: &str, operation: &str) -> Vec<String> {
    let mut args = vec![
        "/usr/bin/ssh".into(),
        "-F".into(),
        "/dev/null".into(),
        "-T".into(),
        "-B".into(),
        interface.into(),
        "-p".into(),
        d.ssh_port.to_string(),
        "-i".into(),
        d.identity_file.to_string_lossy().into_owned(),
        "-l".into(),
        d.ssh_user.clone(),
    ];
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
        ip.to_string(),
        operation.into(),
    ]);
    args
}

fn capture(args: &[String], budget: Duration) -> Result<Vec<u8>, Error> {
    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
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

fn fetch(args: &[String], budget: Duration) -> Result<Directory, Error> {
    Ok(serde_json::from_slice(&capture(args, budget)?)?)
}

fn accept(d: &TrustedDevice, directory: &Directory) -> Result<(), Error> {
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

pub(crate) fn route(ip: Ipv4Addr, interface: &str) -> Result<(), Error> {
    let args = vec![
        "/usr/sbin/ip".into(),
        "-j".into(),
        "route".into(),
        "get".into(),
        ip.to_string(),
    ];
    let values: Vec<Value> = serde_json::from_slice(&capture(&args, Duration::from_secs(1))?)?;
    if values.len() != 1 || values[0]["dev"] != interface {
        return Err(code("catalog_route_interface_changed"));
    }
    Ok(())
}

fn shutdown(d: &Mdns) -> Result<(), Error> {
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

fn daemon(interface: &str) -> Result<Mdns, Error> {
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
    if flags & libc::IFF_UP as u32 == 0 || flags & libc::IFF_MULTICAST as u32 == 0 {
        return Err(code("catalog_interface_unavailable"));
    }
    let d = Mdns {
        daemon: ServiceDaemon::new().map_err(|_| code("catalog_mdns_unavailable"))?,
        closed: Cell::new(false),
    };
    d.disable_interface(IfKind::All)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    d.enable_interface(interface)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    Ok(d)
}

struct Found {
    directory: Directory,
    device: TrustedDevice,
    ip: Ipv4Addr,
}

fn discover(options: &ArgMatches) -> Result<(Value, Vec<Found>), Error> {
    let t = trust(Path::new(required(options, "trust")))?;
    let d = daemon(required(options, "interface"))?;
    let monitor = d.monitor().map_err(|_| code("catalog_mdns_unavailable"))?;
    let events = d
        .browse(SERVICE)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    let end = Instant::now() + Duration::from_secs(*options.get_one::<u64>("seconds").unwrap());
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
                ScopedIp::V4(v)
                    if v.interface_ids()
                        .iter()
                        .any(|i| i.name == required(options, "interface")) =>
                {
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
                route(*ip, required(options, "interface"))?;
                let budget = fetch_end
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(5));
                if budget.is_zero() {
                    return Err(code("catalog_discovery_budget_exhausted"));
                }
                let directory = fetch(
                    &connector(device, *ip, required(options, "interface"), "catalog"),
                    budget,
                )?;
                accept(device, &directory)?;
                Ok(Found {
                    directory,
                    device: device.clone(),
                    ip: *ip,
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
        json!({"devices":devices,"sources":[{"source":"mdns","interface":required(options,"interface"),
        "status":if partial { "partial" } else { "ok" },"rejected":rejected,"truncated":truncated}],
        "scope":"Snapshot of advertised devices on the selected interface. Unavailable sources do not prove absence; channel reachability is not agent liveness."}),
        found,
    ))
}

fn advertise(options: &ArgMatches) -> Result<(), Error> {
    let path = Path::new(required(options, "config"));
    let c = load(path)?;
    current(&c)?;
    let interface = required(options, "interface");
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
    let seconds = *options.get_one::<u64>("seconds").unwrap();
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

fn required<'a>(m: &'a ArgMatches, key: &str) -> &'a str {
    m.get_one::<String>(key).unwrap()
}
fn config_arg() -> Arg {
    Arg::new("config")
        .long("config")
        .required(true)
        .help("Private catalogue JSON; each endpoint fixes its own mailbox and sender.")
}
fn browse_args(c: Cli) -> Cli {
    c.arg(
        Arg::new("trust")
            .long("trust")
            .required(true)
            .help("Private list of known device IDs, mailbox bindings and pinned SSH files."),
    )
    .arg(
        Arg::new("interface")
            .long("interface")
            .required(true)
            .help("Inspect only this local interface."),
    )
    .arg(
        Arg::new("seconds")
            .long("seconds")
            .default_value("5")
            .value_parser(clap::value_parser!(u64).range(1..=30)),
    )
}

pub(crate) fn command() -> Cli {
    Cli::new("catalog").about("Publish profiles and find known devices on one LAN interface.")
        .long_about("Only explicitly published profiles are exported. Device discovery is unauthenticated; profile reads and mailbox calls require pinned SSH identity and a fixed endpoint. Never launches sessions or retries messages.")
        .subcommand_required(true)
        .subcommand(Cli::new("publish").about("Publish an existing peer; replace a profile binding explicitly with --profile-id.")
            .arg(config_arg()).arg(Arg::new("peer").required(true))
            .arg(Arg::new("name").long("name").required(true)).arg(Arg::new("role").long("role").required(true))
            .arg(Arg::new("device_name").long("device-name").default_value("htalk device"))
            .arg(Arg::new("profile_id").long("profile-id"))
            .arg(Arg::new("ssh_port").long("ssh-port").default_value("22").value_parser(clap::value_parser!(u16).range(1..))))
        .subcommand(Cli::new("unpublish").arg(config_arg()).arg(Arg::new("profile_id").required(true)))
        .subcommand(Cli::new("export").about("Read only this endpoint's published profiles; no private session addresses.").arg(config_arg()))
        .subcommand(Cli::new("serve").about("Fixed SSH endpoint: accepts only the exact remote command catalog or mcp.").arg(config_arg()))
        .subcommand(Cli::new("advertise").about("Advertise this catalogue until stopped; no standalone daemon is installed.")
            .arg(config_arg()).arg(Arg::new("interface").long("interface").required(true))
            .arg(Arg::new("seconds").long("seconds").default_value("0").value_parser(clap::value_parser!(u64).range(0..=86400))))
        .subcommand(browse_args(Cli::new("discover").about("Find advertisements; fetch profiles only from known, pinned devices.")))
        .subcommand(browse_args(Cli::new("connect").about("Find this published profile and expose its checked MCP route on stdio.")
            .arg(Arg::new("profile").required(true).help("Exact display name, or profile UUID when names are ambiguous."))))
}

fn emit(v: &Value) -> Result<(), Error> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, v)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

pub(crate) fn run(
    options: &ArgMatches,
    db: Option<&str>,
    actor: Option<&str>,
) -> Result<(), Error> {
    let (name, m) = options.subcommand().unwrap();
    let signals = if matches!(name, "advertise" | "discover" | "connect") {
        let flag = STOP
            .get_or_init(|| Arc::new(AtomicBool::new(false)))
            .clone();
        Some(Signals(vec![
            signal_hook::flag::register(signal_hook::consts::SIGINT, flag.clone())?,
            signal_hook::flag::register(signal_hook::consts::SIGTERM, flag)?,
        ]))
    } else {
        None
    };
    if name != "publish" && (db.is_some() || actor.is_some()) {
        return Err(code("catalog_binding_fixed_omit_db_and_as"));
    }
    match name {
        "publish" => emit(&publish(m, db, actor)?),
        "unpublish" => {
            let path = Path::new(required(m, "config"));
            let _lock = lock(path)?;
            let mut c = load(path)?;
            let id = required(m, "profile_id");
            uuid(id)?;
            let before = c.profiles.len();
            c.profiles.retain(|p| p.profile_id != id);
            if before == c.profiles.len() {
                return Err(code("catalog_unknown_profile"));
            }
            save(path, &c)?;
            emit(&json!({"state":"unpublished","profile_id":id}))
        }
        "export" => emit(&serde_json::to_value(directory(Path::new(required(
            m, "config",
        )))?)?),
        "serve" => {
            let path = Path::new(required(m, "config"));
            match std::env::var("SSH_ORIGINAL_COMMAND").as_deref() {
                Ok("catalog") => emit(&serde_json::to_value(directory(path)?)?),
                Ok("mcp") => {
                    let c = load(path)?;
                    crate::mcp::run_catalog(c.database, c.sender.name, path.into())
                        .map_err(|_| code("catalog_mcp_failed"))
                }
                _ => Err(code("catalog_remote_command_refused")),
            }
        }
        "advertise" => advertise(m),
        "discover" => emit(&discover(m)?.0),
        "connect" => {
            let selector = required(m, "profile");
            text(selector, 128)?;
            let (_, found) = discover(m)?;
            let mut selected = found.into_iter().flat_map(|f| {
                let profiles: Vec<_> = f
                    .directory
                    .profiles
                    .iter()
                    .filter(|p| {
                        (p.profile_id == selector || p.display_name == selector)
                            && p.binding_state == "current"
                    })
                    .map(|p| ProfileBinding {
                        profile_id: p.profile_id.clone(),
                        binding_id: p.binding_id.clone(),
                        peer_name: p.peer_name.clone(),
                    })
                    .collect();
                profiles
                    .into_iter()
                    .map(move |p| (f.device.clone(), f.ip, p))
            });
            let (device, ip, profile) = selected
                .next()
                .ok_or_else(|| code("catalog_profile_unavailable"))?;
            if selected.next().is_some() {
                return Err(code("catalog_ambiguous_profile"));
            }
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
            crate::mcp::connect_catalog(
                connector(&device, ip, required(m, "interface"), "mcp"),
                expected,
                ip,
                required(m, "interface").into(),
            )
            .map_err(|_| code("catalog_mcp_failed"))
        }
        _ => Err(code("invalid_arguments")),
    }
}
