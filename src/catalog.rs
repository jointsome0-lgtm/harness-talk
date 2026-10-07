//! Published profiles on known devices. Discovery never registers or messages peers.
mod endpoint;
mod mailbox;
mod transport;
use crate::{
    commands::{Mailbox, PeerCommand},
    error::Error,
    model::Peer,
    os::{self, Grouped, Open},
    validate,
};
use clap::{Args, Subcommand};
pub(crate) use endpoint::{NO_SCOPE, serve};
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
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use transport::{Pan, Route, TailClient};

const SERVICE: &str = "_htalk._tcp.local.";
const META_KEY: &str = "harness-talk/catalog";
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
        command: &Mailbox,
    ) -> Result<Option<Value>, Error> {
        let allowed = |name: &str| self.profiles.iter().any(|p| p.peer_name == name);
        match command {
            Mailbox::Send { recipient, .. } => {
                if !allowed(recipient) {
                    return Err(code("catalog_profile_not_published"));
                }
            }
            Mailbox::Show { message_id }
            | Mailbox::Ack { message_id }
            | Mailbox::Reply { message_id, .. }
            | Mailbox::Wait { message_id, .. } => {
                let m = mailbox::message(db, message_id, &self.sender)
                    .map_err(|_| code("catalog_conversation_unavailable"))?;
                let other = if m.row.sender == self.sender {
                    &m.row.recipient
                } else {
                    &m.row.sender
                };
                if !allowed(other) {
                    return Err(code("catalog_conversation_unavailable"));
                }
                if matches!(command, Mailbox::Show { .. }) {
                    return Ok(Some(serde_json::to_value(m)?));
                }
            }
            Mailbox::Peer(PeerCommand::Check { name }) => {
                if !allowed(name) {
                    return Err(code("catalog_profile_not_published"));
                }
            }
            Mailbox::Peer(PeerCommand::List { .. }) => {
                return Ok(Some(
                    json!({"peers":self.profiles.iter().map(|p| json!({"name":p.peer_name,"profile_id":p.profile_id,"binding_id":p.binding_id,"runtime_status":"unknown"})).collect::<Vec<_>>(),"retired_hidden":0}),
                ));
            }
            Mailbox::Inbox { limit, after_seq } => {
                return self.page(db, false, limit, after_seq.as_deref(), false);
            }
            Mailbox::Sent {
                limit,
                before_seq,
                bodies,
            } => return self.page(db, true, limit, before_seq.as_deref(), *bodies),
            Mailbox::Peer(_) | Mailbox::Migrate | Mailbox::Watch => {}
        }
        Ok(None)
    }

    fn page(
        &self,
        db: &Path,
        sent: bool,
        limit: &str,
        cursor: Option<&str>,
        bodies: bool,
    ) -> Result<Option<Value>, Error> {
        let names: Vec<_> = self.profiles.iter().map(|p| p.peer_name.clone()).collect();
        let limit = limit
            .parse::<i64>()
            .map_err(|_| code("limit_must_be_between_1_and_500"))?;
        let cursor = cursor
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| code("seq_cursor_must_be_a_positive_integer"))
            })
            .transpose()?;
        let page = mailbox::page(db, &self.sender, &names, sent, limit, cursor, bodies)?;
        let next = page.messages.last().filter(|_| page.omitted > 0).map(|m| {
            format!(
                "htalk {} --limit {} --{} {}{}",
                if sent { "sent" } else { "inbox" },
                limit,
                if sent { "before-seq" } else { "after-seq" },
                m["seq"],
                if bodies { " --bodies" } else { "" }
            )
        });
        let mut value = serde_json::to_value(page)?;
        if let Some(next) = next {
            value["recovery"] = json!({"next_page":next});
        }
        Ok(Some(value))
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
    #[serde(default)]
    tailscale_peer_id: Option<String>,
    #[serde(default)]
    ssh_address: Option<String>,
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

fn stamp(path: &Path) -> Result<DbStamp, Error> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || !os::is_mine(&m) {
        return Err(code("catalog_mailbox_not_owned_file"));
    }
    let (device, inode) = os::file_id(&m);
    Ok(DbStamp { device, inode })
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
    let rows = mailbox::peers(&c.database, &names)?;
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

fn local_binding(path: &Path, db: &Path, sender: &str) -> Result<Binding, Error> {
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

fn read_binding(path: &Path) -> Result<Binding, Error> {
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
    if !os::is_mine(&m) || os::others_write(&m) {
        return Err(code("catalog_directory_not_owned"));
    }
    // Keep normal config writers coordinated with older clients. A .lock config
    // must use a separate inode so acquiring its lock never creates the config.
    let mut lock_path = path.with_extension("lock");
    if lock_path == path {
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        lock_path = name.into();
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .private()
        .no_follow()
        .open(lock_path)?;
    let m = f.metadata()?;
    if !m.is_file() || !os::is_private(&m) {
        return Err(code("catalog_invalid_lock"));
    }
    if !os::try_lock(&f, false).unwrap_or(false) {
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
            .private()
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

fn publish(options: &Publish, db: Option<&str>, actor: Option<&str>) -> Result<Value, Error> {
    let path = PathBuf::from(&options.config);
    let _lock = lock(&path)?;
    let name = options.peer.as_str();
    validate::peer_name(name)?;
    let mut c = if path.exists() {
        load(&path)?
    } else {
        let database = os::resolve(Path::new(
            db.ok_or_else(|| code("catalog_publish_requires_db_and_as"))?,
        ));
        let sender = actor.ok_or_else(|| code("catalog_publish_requires_db_and_as"))?;
        let rows = mailbox::peers(&database, &[sender.into()])?;
        if rows[0].retired_at.is_some() {
            return Err(code("peer_retired"));
        }
        Catalog {
            schema_version: 1,
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: options.device_name.clone(),
            ssh_port: options.ssh_port,
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
    let row = mailbox::peers(&c.database, &[name.into()])?.remove(0);
    if row.retired_at.is_some() {
        return Err(code("peer_retired"));
    }
    let id = options
        .profile_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    uuid(&id)?;
    let display_name = options.name.clone();
    let role = options.role.clone();
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
    let mut tail_ids = HashSet::new();
    for d in &t.devices {
        for id in [&d.device_id, &d.mailbox_id, &d.generation] {
            uuid(id)?;
        }
        validate::peer_name(&d.sender)?;
        if let Some(id) = &d.tailscale_peer_id
            && (!transport::peer_id(id) || !tail_ids.insert(id))
        {
            return Err(code("catalog_invalid_trust"));
        }
        if let Some(address) = &d.ssh_address {
            transport::ssh_address(address)?;
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

fn connector(d: &TrustedDevice, route: &Route, operation: &str) -> Vec<String> {
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
    if !os::multicast_ready(flags) {
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
    route: Route,
}

// Canonical UUID input names an identity and never falls back to a description.
fn select_profile(
    found: Vec<Found>,
    selector: &str,
) -> Result<(TrustedDevice, Route, ProfileBinding), Error> {
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
            .map(move |p| (f.device.clone(), f.route.clone(), p))
    });
    let (device, route, profile) = selected
        .next()
        .ok_or_else(|| code("catalog_profile_unavailable"))?;
    if selected.next().is_some() {
        return Err(code("catalog_ambiguous_profile"));
    }
    Ok((device, route, profile))
}

fn discover(options: &Browse) -> Result<(Value, Vec<Found>), Error> {
    let t = trust(Path::new(&options.trust))?;
    if options.via == "ssh" {
        if options.interface.is_some() {
            return Err(code("catalog_ssh_omit_interface"));
        }
        return discover_ssh(t);
    }
    if options.via == "tailscale" {
        if options.interface.is_some() {
            return Err(code("catalog_tailscale_omit_interface"));
        }
        return discover_tailscale(options, t);
    }
    let pan = if options.via == "bluetooth" {
        Some(Pan::read(options.interface())?)
    } else {
        None
    };
    discover_mdns(options, t, pan)
}

fn discover_ssh(t: Trust) -> Result<(Value, Vec<Found>), Error> {
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
            let route = Route::DirectSsh {
                ip: transport::ssh_address(address)?,
            };
            let budget = end
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            if budget < Duration::from_secs(1) {
                return Err(code("catalog_discovery_budget_exhausted"));
            }
            let directory = fetch(&connector(&device, &route, "catalog"), budget)?;
            accept(&device, &directory)?;
            Ok(Found {
                directory,
                device: device.clone(),
                route,
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

fn discover_tailscale(options: &Browse, t: Trust) -> Result<(Value, Vec<Found>), Error> {
    let client = TailClient::new(&options.tailscale_binary, &options.tailscale_socket)?;
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
            let route = Route::Tailscale {
                ip,
                client: client.clone(),
                local_id: status.local_id.clone(),
                peer_id: id.clone(),
            };
            route.check()?;
            let budget = end
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            if budget.is_zero() {
                return Err(code("catalog_discovery_budget_exhausted"));
            }
            let directory = fetch(&connector(&device, &route, "catalog"), budget)?;
            accept(&device, &directory)?;
            Ok(Found {
                directory,
                device: device.clone(),
                route,
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

fn discover_mdns(
    options: &Browse,
    t: Trust,
    pan: Option<Pan>,
) -> Result<(Value, Vec<Found>), Error> {
    let d = daemon(options.interface())?;
    let monitor = d.monitor().map_err(|_| code("catalog_mdns_unavailable"))?;
    let events = d
        .browse(SERVICE)
        .map_err(|_| code("catalog_mdns_unavailable"))?;
    let end = Instant::now() + Duration::from_secs(options.seconds);
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
                        .any(|i| i.name == options.interface()) =>
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
                let route = Route::Interface {
                    ip: *ip,
                    interface: options.interface().into(),
                    pan: pan.clone(),
                };
                route.check()?;
                let budget = fetch_end
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(5));
                if budget.is_zero() {
                    return Err(code("catalog_discovery_budget_exhausted"));
                }
                let directory = fetch(&connector(device, &route, "catalog"), budget)?;
                accept(device, &directory)?;
                Ok(Found {
                    directory,
                    device: device.clone(),
                    route,
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
        json!({"devices":devices,"sources":[{"source":"mdns","interface":options.interface(),"via":options.via,
        "status":if partial { "partial" } else { "ok" },"rejected":rejected,"truncated":truncated}],
        "scope":"Snapshot of advertised devices on the selected interface. Unavailable sources do not prove absence; channel reachability is not agent liveness."}),
        found,
    ))
}

fn advertise(config: &str, interface: &str, seconds: u64) -> Result<(), Error> {
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
        Action::Publish(options) => emit(&publish(options, db, actor)?),
        Action::Unpublish { config, profile_id } => {
            let path = Path::new(config);
            let _lock = lock(path)?;
            let mut c = load(path)?;
            uuid(profile_id)?;
            let before = c.profiles.len();
            c.profiles.retain(|p| &p.profile_id != profile_id);
            if before == c.profiles.len() {
                return Err(code("catalog_unknown_profile"));
            }
            save(path, &c)?;
            emit(&json!({"state":"unpublished","profile_id":profile_id}))
        }
        Action::Export { config } => emit(&serde_json::to_value(directory(Path::new(config))?)?),
        Action::Serve { config } => {
            let path = Path::new(config);
            match std::env::var("SSH_ORIGINAL_COMMAND").as_deref() {
                Ok("catalog") => emit(&serde_json::to_value(directory(path)?)?),
                Ok("mcp") => {
                    let c = load(path)?;
                    endpoint::published(c.database, c.sender.name, path.into())
                        .map_err(|_| code("catalog_mcp_failed"))
                }
                _ => Err(code("catalog_remote_command_refused")),
            }
        }
        Action::Advertise {
            config,
            interface,
            seconds,
        } => advertise(config, interface, *seconds),
        Action::Discover(browse) => emit(&discover(browse)?.0),
        Action::Connect { profile, browse } => {
            text(profile, 128)?;
            let (_, found) = discover(browse)?;
            let (device, route, profile) = select_profile(found, profile)?;
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
            endpoint::checked(connector(&device, &route, "mcp"), expected, Some(route))
                .map_err(|_| code("catalog_mcp_failed"))
        }
    }
}
