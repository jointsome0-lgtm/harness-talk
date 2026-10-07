//! The catalogue's MCP endpoints. A published mailbox proves on every call that the
//! catalogue still binds its database and sender, and stays inside the published
//! conversations. A checked connection reaches only the endpoint it selected.
use super::{Binding, META_KEY, Route, local_binding, read_binding};
use crate::{
    commands::{self, Arguments, Mailbox},
    mcp::{self, Backend, Checks, Plain, Reply, Server, answer, error},
    os,
};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub(crate) const NO_SCOPE: &str = "This endpoint does not accept a catalogue scope.";
const CHANGED: &str =
    "Catalogue binding changed; no mailbox command was sent. Discover the profile again.";

/// `mcp --catalog` and `mcp --connect --expect-catalog`: the endpoint these options ask for.
pub(crate) fn serve(plain: Plain, options: &commands::Mcp) -> Result<(), Box<dyn Error>> {
    match (plain, &options.catalog, &options.expect_catalog) {
        (Plain::Local { db, peer }, Some(path), _) => published(db, peer, path.into()),
        (Plain::Connect(connector), _, Some(path)) => {
            checked(connector, read_binding(Path::new(path))?, None)
        }
        (plain, ..) => plain.serve(),
    }
}

pub(super) fn published(db: PathBuf, peer: String, path: PathBuf) -> Result<(), Box<dyn Error>> {
    let binding = local_binding(&path, &db, &peer)?;
    mcp::serve(Published {
        db: os::resolve(&db),
        peer,
        path,
        binding,
    })
}

pub(super) fn checked(
    connector: Vec<String>,
    binding: Binding,
    route: Option<Route>,
) -> Result<(), Box<dyn Error>> {
    mcp::serve(Checked {
        connector,
        expected: Arc::new(Expected { binding, route }),
    })
}

fn meta(binding: &Binding) -> Option<(&'static str, Value)> {
    Some((META_KEY, serde_json::to_value(binding).unwrap()))
}

struct Published {
    db: PathBuf,
    peer: String,
    path: PathBuf,
    binding: Binding,
}

impl Backend for Published {
    fn instructions(&self) -> String {
        mcp::local_peer(&self.peer)
    }

    fn meta(&self) -> Option<(&'static str, Value)> {
        meta(&self.binding)
    }

    fn call(
        &self,
        server: &Server,
        command: &Mailbox,
        arguments: Arguments,
        cancel: CancellationToken,
    ) -> Reply {
        let Ok(current) = local_binding(&self.path, &self.db, &self.peer) else {
            return answer(error(CHANGED));
        };
        let current = serde_json::to_value(&current).unwrap();
        if !self.binding.matches(Some(&current))
            || arguments
                .scope
                .as_ref()
                .is_some_and(|selected| !selected.matches(Some(&current)))
        {
            return answer(error(CHANGED));
        }
        let scope = arguments.scope.as_ref().unwrap_or(&self.binding);
        match scope.command_scope(&self.db, command) {
            Ok(Some(value)) => answer(CallToolResult::structured(
                json!({"exit_code":0,"result":value}),
            )),
            Ok(None) => server.local(&self.db, &self.peer, arguments.args, cancel),
            Err(e) => answer(error(e.to_string())),
        }
    }
}

struct Checked {
    connector: Vec<String>,
    expected: Arc<Expected>,
}

impl Backend for Checked {
    fn instructions(&self) -> String {
        let selected = format!(
            " Selected profiles: {}.",
            self.expected.binding.profile_names().join(", ")
        );
        format!("{}{selected}", mcp::REMOTE)
    }

    fn meta(&self) -> Option<(&'static str, Value)> {
        meta(&self.expected.binding)
    }

    fn call(
        &self,
        server: &Server,
        _command: &Mailbox,
        arguments: Arguments,
        cancel: CancellationToken,
    ) -> Reply {
        match arguments.unscoped() {
            Ok(args) => server.remote(self.connector.clone(), args, cancel, self.expected.clone()),
            Err(refusal) => answer(error(refusal)),
        }
    }
}

/// What a checked connection holds the remote endpoint to.
struct Expected {
    binding: Binding,
    route: Option<Route>,
}

impl Checks for Expected {
    fn before(&self) -> Result<(), &'static str> {
        if self.route.as_ref().is_some_and(|r| r.check().is_err()) {
            return Err(
                "The selected channel binding is unavailable or changed; no mailbox command was sent. Discover the profile again.",
            );
        }
        Ok(())
    }

    fn endpoint(&self, meta: Option<&Map<String, Value>>) -> bool {
        self.binding.matches(meta.and_then(|m| m.get(META_KEY)))
    }

    fn extend(&self, arguments: &mut Map<String, Value>) {
        arguments.insert(
            "_catalog_binding".into(),
            serde_json::to_value(&self.binding).unwrap(),
        );
    }
}
