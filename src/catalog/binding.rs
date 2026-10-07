//! What an endpoint is held to: one device, mailbox, sender and the profiles it published.
use super::{
    code, mailbox,
    profile::{directory, load},
    read, uuid,
};
use crate::{
    commands::{Mailbox, PeerCommand},
    error::Error,
    os, validate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileBinding {
    pub(super) profile_id: String,
    pub(super) binding_id: String,
    pub(super) peer_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub(super) schema_version: u8,
    pub(super) device_id: String,
    pub(super) mailbox_id: String,
    pub(super) generation: String,
    pub(super) sender: String,
    pub(super) profiles: Vec<ProfileBinding>,
}

impl Binding {
    pub(super) fn validate(&self) -> Result<(), Error> {
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

pub(super) fn local_binding(path: &Path, db: &Path, sender: &str) -> Result<Binding, Error> {
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

pub(super) fn read_binding(path: &Path) -> Result<Binding, Error> {
    let b: Binding = read(path)?;
    b.validate()?;
    if b.profiles.is_empty() {
        return Err(code("catalog_invalid_binding"));
    }
    Ok(b)
}
