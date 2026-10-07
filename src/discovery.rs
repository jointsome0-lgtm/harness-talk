//! `peer discover`: ask each harness for the sessions it can see, then filter and sort
//! them. Discovery never registers or wakes a peer.
use crate::{
    adapters::{self, Harness, Query},
    error::Error,
    model::Found,
    os,
};
use serde_json::{Value, json};
use std::path::Path;

/// A validated (canonical UUID, absolute workspace) pair, as saved by registration.
#[cfg(native_clients)]
pub(crate) fn address(
    session_id: Option<&Value>,
    workspace: Option<&Value>,
) -> Option<(String, String)> {
    let session_id = crate::validate::uuid(session_id?.as_str()?).ok()?;
    let workspace = workspace?.as_str().filter(|w| Path::new(w).is_absolute())?;
    Some((session_id, workspace.to_owned()))
}

#[cfg(native_clients)]
pub(crate) fn source(fields: Value) -> serde_json::Map<String, Value> {
    match fields {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

pub(crate) fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Native paths match the already-resolved requested workspace after resolution.
fn in_workspace(native: Option<&Value>, workspace: &str) -> bool {
    native
        .and_then(Value::as_str)
        .filter(|p| Path::new(p).is_absolute())
        .is_some_and(|p| lossy(&os::resolve(Path::new(p))) == workspace)
}

/// Filter by the resolved workspace, sort, and add the public guidance fields.
pub fn finish(found: Found, workspace: Option<&str>) -> Value {
    let mut sessions = found.sessions;
    if let Some(workspace) = workspace {
        sessions.retain(|row| in_workspace(row.get("workspace"), workspace));
    }
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    sessions.sort_by_key(|row| {
        (
            text(row, "harness"),
            text(row, "workspace"),
            text(row, "session_id"),
        )
    });
    json!({"sessions": sessions, "sources": found.sources,
        "scope": "Current local client environment and the listed endpoints only. Unavailable sources do not prove there are no sessions."})
}

pub(crate) fn discover(
    harness: Option<Harness>,
    workspace: Option<&str>,
    codex_sockets: Option<&[String]>,
    opencode_urls: Option<&[String]>,
) -> Result<Value, Error> {
    let workspace = workspace.map(|w| lossy(&os::resolve(Path::new(w))));
    let query = Query {
        workspace: workspace.as_deref(),
        codex_sockets,
        opencode_urls,
    };
    query.check(harness)?;
    let mut found = Found::default();
    for (each, adapter) in adapters::all() {
        if harness.is_none_or(|h| h == each) {
            let part = adapter.discover(&query);
            found.sessions.extend(part.sessions);
            found.sources.extend(part.sources);
        }
    }
    Ok(finish(found, query.workspace))
}
