//! The catalogue file: one mailbox and sender, and the peers published from it as profiles.
use super::{LIMIT, Publish, code, emit, mailbox, read, text, uuid};
use crate::{
    error::Error,
    model::Peer,
    os::{self, Open},
    validate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

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
pub(super) struct Catalog {
    schema_version: u8,
    pub(super) device_id: String,
    device_name: String,
    #[serde(default = "super::ssh_port")]
    pub(super) ssh_port: u16,
    mailbox_id: String,
    generation: String,
    pub(super) database: PathBuf,
    database_stamp: DbStamp,
    pub(super) sender: Peer,
    profiles: Vec<Published>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    pub(super) profile_id: String,
    pub(super) display_name: String,
    pub(super) role: String,
    pub(super) binding_id: String,
    pub(super) peer_name: String,
    pub(super) harness: String,
    pub(super) delivery: String,
    pub(super) binding_state: String,
    pub(super) runtime_status: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Directory {
    pub(super) schema_version: u8,
    pub(super) device_id: String,
    pub(super) device_name: String,
    pub(super) mailbox_id: String,
    pub(super) generation: String,
    pub(super) sender: String,
    pub(super) profiles: Vec<Profile>,
}

fn stamp(path: &Path) -> Result<DbStamp, Error> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || !os::is_mine(&m) {
        return Err(code("catalog_mailbox_not_owned_file"));
    }
    let (device, inode) = os::file_id(&m);
    Ok(DbStamp { device, inode })
}

pub(super) fn load(path: &Path) -> Result<Catalog, Error> {
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

pub(super) fn current(c: &Catalog) -> Result<Vec<Peer>, Error> {
    let mut names = vec![c.sender.name.clone()];
    names.extend(c.profiles.iter().map(|p| p.peer.name.clone()));
    let rows = mailbox::peers(&c.database, &names)?;
    if rows[0] != c.sender || rows[0].retired_at.is_some() {
        return Err(code("catalog_sender_binding_changed"));
    }
    Ok(rows.into_iter().skip(1).collect())
}

pub(super) fn directory(path: &Path) -> Result<Directory, Error> {
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

pub(super) fn publish(
    options: &Publish,
    db: Option<&str>,
    actor: Option<&str>,
) -> Result<Value, Error> {
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

// The answer is written under the lock, as `catalog unpublish` always did.
pub(super) fn unpublish(path: &Path, profile_id: &str) -> Result<(), Error> {
    let _lock = lock(path)?;
    let mut c = load(path)?;
    uuid(profile_id)?;
    let before = c.profiles.len();
    c.profiles.retain(|p| p.profile_id != profile_id);
    if before == c.profiles.len() {
        return Err(code("catalog_unknown_profile"));
    }
    save(path, &c)?;
    emit(&json!({"state":"unpublished","profile_id":profile_id}))
}
