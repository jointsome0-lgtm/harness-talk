//! The adapter contract and the registry. A harness is one module beside this file, its
//! lines in the registry below and its page in `docs/`. The rest of the program reaches a
//! harness only through `adapter()` and `all()`.
#[cfg(native_clients)]
pub mod claude;
#[cfg(native_clients)]
pub mod codex;
pub mod opencode;

use crate::{
    error::Error,
    model::{Cleanup, CleanupStatus, Found, Message, NativePeer, Outcome, Skip},
    os,
};
use serde_json::Value;
use std::{fmt, path::Path};

/// What the program asks of a harness. Only fixed codes and class names come back as
/// details, so foreign error text, paths and credentials never reach the mailbox or the output.
pub(crate) trait Adapter: Sync {
    /// Checks the address given to `peer add`, before the mailbox is opened.
    fn address(
        &self,
        session: &str,
        workspace: &str,
        socket: Option<&str>,
        url: Option<&str>,
    ) -> Result<Address, Error>;

    /// Submits one notification, unless `skip` says at the last moment that none is needed.
    fn notify(&self, peer: &NativePeer, message: &Message, body: &str, skip: Skip<'_>) -> Outcome;

    /// Takes back the notification of a message its recipient acknowledged.
    fn dismiss(&self, _peer: &NativePeer, _message: &Message) -> Cleanup {
        Cleanup::new(CleanupStatus::Unsupported).detail("client_has_no_notification_removal")
    }

    /// Reads the live session at the address without waking it.
    fn probe(&self, peer: &NativePeer) -> Result<Value, Error>;

    /// Lists the sessions it can see for `peer discover`, with the sources it asked.
    fn discover(&self, query: &Query<'_>) -> Found;
}

// The registry: a new harness is a variant, its name, its adapter and its place in `ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Codex,
    Claude,
    Opencode,
}
impl Harness {
    /// In the order `peer discover` lists their sources.
    const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Opencode];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
        }
    }
}
pub(crate) fn adapter(harness: Harness) -> &'static dyn Adapter {
    match harness {
        #[cfg(native_clients)]
        Harness::Codex => &codex::Codex,
        #[cfg(native_clients)]
        Harness::Claude => &claude::Claude,
        #[cfg(not(native_clients))]
        Harness::Codex => &NotPorted(Harness::Codex),
        #[cfg(not(native_clients))]
        Harness::Claude => &NotPorted(Harness::Claude),
        Harness::Opencode => &opencode::Opencode,
    }
}
pub(crate) fn all() -> impl Iterator<Item = (Harness, &'static dyn Adapter)> {
    Harness::ALL.into_iter().map(|h| (h, adapter(h)))
}
impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl std::str::FromStr for Harness {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|h| h.as_str() == s)
            .ok_or_else(|| Error::code("unsupported_harness"))
    }
}

/// `htalk receive` bridges a remote watch into a Codex session. No other harness has one.
#[cfg(native_clients)]
pub(crate) use codex::receive::run as receive;
#[cfg(not(native_clients))]
pub(crate) fn receive(_: crate::commands::Receive) -> Result<(), Error> {
    Err(Error::not_ported())
}

/// Stands for a harness on a system its adapter is not ported to. Nothing is submitted and
/// no client is asked.
#[cfg(not(native_clients))]
struct NotPorted(Harness);
#[cfg(not(native_clients))]
impl Adapter for NotPorted {
    fn address(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<Address, Error> {
        Err(Error::not_ported())
    }
    fn notify(&self, _: &NativePeer, _: &Message, _: &str, _: Skip<'_>) -> Outcome {
        Outcome::not_submitted(Error::not_ported().fixed())
    }
    fn dismiss(&self, _: &NativePeer, _: &Message) -> Cleanup {
        Cleanup::new(CleanupStatus::Unsupported).detail(Error::not_ported().fixed())
    }
    fn probe(&self, _: &NativePeer) -> Result<Value, Error> {
        Err(Error::not_ported())
    }
    fn discover(&self, _: &Query<'_>) -> Found {
        Found {
            sessions: Vec::new(),
            sources: vec![
                serde_json::json!({"harness": self.0.as_str(), "status": "unavailable",
                "detail": Error::not_ported().fixed()}),
            ],
        }
    }
}

/// A native address as the mailbox keeps it.
pub(crate) struct Address {
    pub session_id: String,
    pub workspace: String,
    pub socket: Option<String>,
    pub url: Option<String>,
}

/// The canonical workspace of an address. It must be a directory.
fn workspace(given: &str) -> Result<String, Error> {
    let workspace = os::resolve_strict(Path::new(given))?;
    if !workspace.is_dir() {
        return Err(Error::code("workspace_must_be_a_directory"));
    }
    Ok(workspace.to_string_lossy().into_owned())
}

/// What `peer discover` was asked.
pub(crate) struct Query<'a> {
    /// Already resolved. The caller filters by it; a harness may use it to ask for less.
    pub workspace: Option<&'a str>,
    pub codex_sockets: Option<&'a [String]>,
    pub opencode_urls: Option<&'a [String]>,
}
impl Query<'_> {
    /// An endpoint option of one harness is refused when another harness is searched.
    pub(crate) fn check(&self, harness: Option<Harness>) -> Result<(), Error> {
        let other = |own| harness.is_some_and(|h| h != own);
        if self.codex_sockets.is_some() && other(Harness::Codex) {
            return Err(Error::code("codex_socket_requires_codex_discovery"));
        }
        if self.opencode_urls.is_some() && other(Harness::Opencode) {
            return Err(Error::code("opencode_url_requires_opencode_discovery"));
        }
        Ok(())
    }
}
