//! Read native session metadata. Discovery never registers or wakes a peer.
use crate::{
    claude,
    codex::{rpc::Rpc, state},
    error::Failure,
    model::{Found, Harness},
    opencode, os, validate,
};
use rusqlite::{OpenFlags, types::ValueRef};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One JSON RPC call on a connected Codex app-server.
pub type Call<'a> = Box<dyn FnMut(&str, Value) -> Result<Value, Failure> + 'a>;

const PAGE_LIMIT: u64 = 100;
const MAX_IDS: usize = 200;
const MAX_CURSORS: usize = 20;
const SCAN_BUDGET: Duration = Duration::from_secs(15);

/// A validated (canonical UUID, absolute workspace) pair, as saved by registration.
fn address(session_id: Option<&Value>, workspace: Option<&Value>) -> Option<(String, String)> {
    let session_id = validate::uuid(session_id?.as_str()?).ok()?;
    let workspace = workspace?.as_str().filter(|w| Path::new(w).is_absolute())?;
    Some((session_id, workspace.to_owned()))
}

fn source(fields: Value) -> Map<String, Value> {
    match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Verify each `claude agents --json` row against its session metadata and live socket.
/// A malformed row is counted as rejected and never hides another verified row.
pub fn claude_sessions(
    listed: Result<Vec<Value>, Failure>,
    metadata: &dyn Fn(i64) -> Result<Value, Failure>,
) -> Found {
    let mut src = source(json!({"harness": "claude", "source": "claude_agents", "status": "ok"}));
    let mut sessions = Vec::new();
    match listed {
        Err(failure) => {
            // The bare class name, even for htalk's own coded ValueErrors.
            src.insert("status".into(), json!("unavailable"));
            src.insert("detail".into(), json!(failure.class_name()));
        }
        Ok(rows) => {
            let mut identities: HashMap<&str, usize> = HashMap::new();
            for row in &rows {
                if let Some(id) = row.get("sessionId").and_then(Value::as_str) {
                    *identities.entry(id).or_default() += 1;
                }
            }
            let mut rejected = 0u64;
            for row in &rows {
                match verify_claude_row(row, &identities, metadata) {
                    Some(session) => sessions.push(session),
                    None => rejected += 1,
                }
            }
            if rejected > 0 {
                src.insert("status".into(), json!("partial"));
                src.insert("rejected".into(), json!(rejected));
                src.insert(
                    "detail".into(),
                    json!(
                        "Some live records could not be verified; run discovery again to refresh."
                    ),
                );
            }
        }
    }
    Found {
        sessions,
        sources: vec![Value::Object(src)],
    }
}

fn verify_claude_row(
    row: &Value,
    identities: &HashMap<&str, usize>,
    metadata: &dyn Fn(i64) -> Result<Value, Failure>,
) -> Option<Value> {
    let row = row.as_object()?;
    let (session_id, workspace) = address(Some(row.get("sessionId")?), Some(row.get("cwd")?))?;
    // Integers only: JSON floats and booleans are not PIDs.
    let pid = row.get("pid")?.as_i64().filter(|p| *p > 0)?;
    if identities.get(row.get("sessionId")?.as_str()?).copied() != Some(1) {
        return None;
    }
    let saved = metadata(pid).ok()?;
    let saved = saved.as_object()?;
    if saved.get("sessionId").and_then(Value::as_str) != Some(session_id.as_str())
        || saved.get("cwd").and_then(Value::as_str) != Some(workspace.as_str())
    {
        return None;
    }
    os::owned_socket(Path::new(saved.get("messagingSocketPath")?.as_str()?)).ok()?;
    Some(
        json!({"harness": "claude", "session_id": session_id, "workspace": workspace,
                "runtime_status": "running", "source": "claude_agents", "pid": pid}),
    )
}

pub fn default_codex_socket() -> PathBuf {
    state::codex_home().join("app-server-control/app-server-control.sock")
}

fn dev_major(dev: u64) -> u64 {
    u64::from(libc::major(dev))
}
fn dev_minor(dev: u64) -> u64 {
    u64::from(libc::minor(dev))
}

fn int(text: &str, radix: u32) -> Result<i64, Failure> {
    i64::from_str_radix(text, radix).map_err(|_| Failure::Class("ValueError"))
}

/// Text column as Python's sqlite3 would decode it; undecodable text aborts the query.
fn column(value: ValueRef<'_>) -> Result<Value, Failure> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => {
            json!(std::str::from_utf8(t).map_err(|_| Failure::Class("OperationalError"))?)
        }
        ValueRef::Blob(_) => json!({"blob": true}),
    })
}

/// Linux kernel lock evidence for native CLI threads, without taking a lock.
/// `directory` is `$CODEX_HOME/thread-writer-locks`; `lock_table` is normally `/proc/locks`.
pub fn codex_writers(
    directory: &Path,
    lock_table: &Path,
    state_path: impl FnOnce() -> Result<PathBuf, Failure>,
) -> Found {
    let mut src = source(
        json!({"harness": "codex", "source": "codex_writer_locks", "status": "ok",
        "path": lossy(directory), "scope": "CLI writers visible in this Linux process namespace."}),
    );
    let mut sessions = Vec::new();
    if let Err(failure) = scan_writers(directory, lock_table, state_path, &mut src, &mut sessions) {
        src.insert(
            "status".into(),
            json!(if sessions.is_empty() {
                "unavailable"
            } else {
                "partial"
            }),
        );
        src.insert("detail".into(), json!(failure.class_name()));
    }
    Found {
        sessions,
        sources: vec![Value::Object(src)],
    }
}

fn scan_writers(
    directory: &Path,
    lock_table: &Path,
    state_path: impl FnOnce() -> Result<PathBuf, Failure>,
    src: &mut Map<String, Value>,
    sessions: &mut Vec<Value>,
) -> Result<(), Failure> {
    let uid = unsafe { libc::getuid() };
    let mut files: HashMap<(u64, u64, u64), String> = HashMap::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Some(stem) = name.strip_suffix(".lock").filter(|s| !s.is_empty()) else {
            continue;
        };
        let Ok(ident) = validate::uuid(stem) else {
            continue;
        };
        let Ok(info) = path.symlink_metadata() else {
            continue;
        };
        if info.file_type().is_file() && info.uid() == uid {
            files.insert(
                (dev_major(info.dev()), dev_minor(info.dev()), info.ino()),
                ident,
            );
        }
    }
    let table = std::fs::read(lock_table)?;
    let table = String::from_utf8(table).map_err(|_| Failure::Class("UnicodeDecodeError"))?;
    let mut held: Vec<(String, i64)> = Vec::new();
    for line in table.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // Ignore blocked lock requests ("->"), read locks and unrelated lock types.
        if fields.len() < 6 || fields[1..4] != ["FLOCK", "ADVISORY", "WRITE"] {
            continue;
        }
        let parts: Vec<&str> = fields[5].split(':').collect();
        let [major, minor, inode] = parts[..] else {
            return Err(Failure::Class("ValueError"));
        };
        let (major, minor, inode) = (int(major, 16)?, int(minor, 16)?, int(inode, 10)?);
        let (Ok(major), Ok(minor), Ok(inode)) = (
            u64::try_from(major),
            u64::try_from(minor),
            u64::try_from(inode),
        ) else {
            continue;
        };
        let Some(ident) = files.get(&(major, minor, inode)) else {
            continue;
        };
        let pid = int(fields[4], 10)?;
        if pid > 0 {
            match held.iter_mut().find(|(id, _)| id == ident) {
                Some(slot) => slot.1 = pid,
                None => held.push((ident.clone(), pid)),
            }
        }
    }
    if held.is_empty() {
        return Ok(());
    }
    let metadata = state_path()?;
    if !metadata.is_absolute() {
        return Err(Failure::Class("ValueError"));
    }
    let db = rusqlite::Connection::open_with_flags(
        &metadata,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(3))?;
    let mut query = db.prepare("SELECT id, cwd, archived, source FROM threads WHERE id=?")?;
    for (ident, pid) in held {
        let mut rows = query.query([&ident])?;
        let Some(row) = rows.next()? else {
            src.insert("status".into(), json!("partial"));
            src.insert(
                "detail".into(),
                json!("Some held writers have no saved address yet."),
            );
            continue;
        };
        let values = (0..4)
            .map(|i| column(row.get_ref(i)?))
            .collect::<Result<Vec<_>, Failure>>()?;
        let unarchived = values[2].as_f64().is_some_and(|a| a == 0.0);
        if !unarchived || values[3] != "cli" {
            continue;
        }
        let Some((session_id, workspace)) = address(Some(&values[0]), Some(&values[1])) else {
            let rejected = src.get("rejected").and_then(Value::as_u64).unwrap_or(0) + 1;
            src.insert("status".into(), json!("partial"));
            src.insert(
                "detail".into(),
                json!("Some held writers have an invalid saved address."),
            );
            src.insert("rejected".into(), json!(rejected));
            continue;
        };
        sessions.push(
            json!({"harness": "codex", "session_id": session_id, "workspace": workspace,
            "runtime_status": "writer_active", "runtime_reason": "kernel_writer_lock",
            "source": "codex_writer_locks", "pid": pid, "socket": null}),
        );
    }
    Ok(())
}

/// Whether Python would have raised a per-record ValueError/KeyError/TypeError rather than
/// an OSError (transport, timeout or scan limit) that ends the whole source.
fn per_record(failure: &Failure) -> bool {
    match failure {
        Failure::Coded(code) => !matches!(
            code.as_str(),
            "codex_websocket_failure"
                | "codex_rpc_timeout"
                | "codex_rpc_closed"
                | "codex_discovery_limit"
                | "codex_discovery_time_limit"
        ),
        Failure::Class(name) => matches!(
            *name,
            "ValueError"
                | "JSONDecodeError"
                | "UnicodeDecodeError"
                | "KeyError"
                | "TypeError"
                | "AttributeError"
        ),
    }
}

fn reject() -> Failure {
    Failure::Class("ValueError")
}

fn connect_codex(path: &Path) -> Result<Call<'static>, Failure> {
    os::owned_socket(path)?;
    let mut rpc = Rpc::connect_unix(path)?;
    Ok(Box::new(move |method, params| rpc.call(method, params)))
}

/// Page `thread/loaded/list` on each distinct app-server socket; `connect` is the transport.
pub fn codex_app_servers(
    paths: &[String],
    connect: &dyn Fn(&Path) -> Result<Call<'static>, Failure>,
) -> Found {
    let mut found = Found::default();
    let mut unique: Vec<String> = Vec::new();
    for path in paths {
        let path = lossy(&os::resolve(Path::new(path)));
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    for path in unique {
        let mut src = source(
            json!({"harness": "codex", "source": "codex_app_server", "socket": path, "status": "ok"}),
        );
        let mut count = 0usize;
        let result = connect(Path::new(&path))
            .and_then(|mut call| scan_loaded(&mut call, &path, &mut found.sessions, &mut count));
        match result {
            Ok(0) => {}
            Ok(rejected) => {
                src.insert("status".into(), json!("partial"));
                src.insert("rejected".into(), json!(rejected));
                src.insert(
                    "detail".into(),
                    json!("Some loaded records could not be verified."),
                );
            }
            Err(failure) => {
                src.insert(
                    "status".into(),
                    json!(if count > 0 { "partial" } else { "unavailable" }),
                );
                src.insert("detail".into(), json!(failure.to_string()));
            }
        }
        if src["status"] == "unavailable" {
            src.insert("next_action".into(), json!("Check the running Codex app-server socket and permissions, or pass --codex-socket. \
                Embedded clients without a socket are outside this source's coverage."));
        }
        found.sources.push(Value::Object(src));
    }
    found
}

/// Returns the number of rejected records; verified sessions are kept even when it fails later.
fn scan_loaded(
    call: &mut Call<'_>,
    socket: &str,
    sessions: &mut Vec<Value>,
    count: &mut usize,
) -> Result<usize, Failure> {
    let deadline = Instant::now() + SCAN_BUDGET;
    let (mut cursor, mut seen_cursors, mut seen_ids) =
        (Value::Null, HashSet::new(), HashSet::new());
    let mut rejected = 0;
    loop {
        if Instant::now() >= deadline {
            return Err(Failure::coded("codex_discovery_time_limit"));
        }
        let page = call(
            "thread/loaded/list",
            json!({"cursor": cursor, "limit": PAGE_LIMIT}),
        )?;
        let Some(data) = page
            .get("data")
            .and_then(Value::as_array)
            .filter(|_| page.is_object())
        else {
            return Err(Failure::coded("invalid_loaded_threads_response"));
        };
        for item in data {
            if seen_ids.len() >= MAX_IDS || Instant::now() >= deadline {
                return Err(Failure::coded("codex_discovery_limit"));
            }
            let Some(ident) = item.as_str().and_then(|s| validate::uuid(s).ok()) else {
                rejected += 1;
                continue;
            };
            if !seen_ids.insert(ident.clone()) {
                continue;
            }
            match loaded_thread(call, &ident, socket) {
                Ok(Some(session)) => {
                    sessions.push(session);
                    *count += 1;
                }
                Ok(None) => {}
                Err(failure) if per_record(&failure) => rejected += 1,
                Err(failure) => return Err(failure),
            }
        }
        match page.get("nextCursor") {
            None | Some(Value::Null) => break,
            Some(Value::String(next)) if !seen_cursors.contains(next) => {
                if seen_cursors.len() >= MAX_CURSORS {
                    return Err(Failure::coded("codex_discovery_limit"));
                }
                seen_cursors.insert(next.clone());
                cursor = json!(next);
            }
            Some(_) => return Err(Failure::coded("invalid_discovery_cursor")),
        }
    }
    Ok(rejected)
}

fn loaded_thread(call: &mut Call<'_>, ident: &str, socket: &str) -> Result<Option<Value>, Failure> {
    let result = call(
        "thread/read",
        json!({"threadId": ident, "includeTurns": false}),
    )?;
    let thread = result
        .as_object()
        .and_then(|r| r.get("thread"))
        .and_then(Value::as_object)
        .ok_or_else(reject)?;
    let (session_id, workspace) =
        address(thread.get("id"), thread.get("cwd")).ok_or_else(reject)?;
    if session_id != ident {
        return Err(reject());
    }
    let status = thread
        .get("status")
        .and_then(Value::as_object)
        .and_then(|s| s.get("type"))
        .ok_or_else(reject)?;
    if status == "notLoaded" {
        return Ok(None);
    } // It was unloaded between list and read.
    if thread.get("canAcceptDirectInput") == Some(&Value::Bool(false)) {
        return Ok(None);
    } // Internal workers belong to their controlling client.
    let Some(status) = status
        .as_str()
        .filter(|s| matches!(*s, "idle" | "active" | "systemError"))
    else {
        return Err(reject());
    };
    Ok(Some(
        json!({"harness": "codex", "session_id": session_id, "workspace": workspace,
        "runtime_status": status, "source": "codex_app_server", "socket": socket}),
    ))
}

/// Add kernel-lock writers that the app-server sources did not already report.
pub fn with_writers(mut app: Found, writers: Option<Found>) -> Found {
    if let Some(writers) = writers {
        let key = |row: &Value| {
            (
                row.get("session_id").cloned(),
                row.get("workspace").cloned(),
            )
        };
        let known: HashSet<_> = app.sessions.iter().map(key).collect();
        app.sessions.extend(
            writers
                .sessions
                .into_iter()
                .filter(|row| !known.contains(&key(row))),
        );
        app.sources.extend(writers.sources);
    }
    app
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
        "next_action": "Choose an exact session address and register it with peer add. Discovery does not register or notify anyone.",
        "scope": "Current local client environment and the listed endpoints only. Unavailable sources do not prove there are no sessions."})
}

fn extend(all: &mut Found, part: Found) {
    all.sessions.extend(part.sessions);
    all.sources.extend(part.sources);
}

pub fn discover(
    h: Option<Harness>,
    workspace: Option<&str>,
    codex_sockets: Option<&[String]>,
    opencode_urls: Option<&[String]>,
) -> Value {
    let workspace = workspace.map(|w| lossy(&os::resolve(Path::new(w))));
    let wants = |harness| h.is_none_or(|h| h == harness);
    let mut found = Found::default();
    if wants(Harness::Claude) {
        extend(
            &mut found,
            claude_sessions(claude::agents(), &claude::session_metadata),
        );
    }
    if wants(Harness::Codex) {
        let paths = codex_sockets
            .map(<[String]>::to_vec)
            .unwrap_or_else(|| vec![lossy(&default_codex_socket())]);
        let app = codex_app_servers(&paths, &connect_codex);
        let writers = codex_sockets.is_none().then(|| {
            codex_writers(
                &state::codex_home().join("thread-writer-locks"),
                Path::new("/proc/locks"),
                state::state_path,
            )
        });
        extend(&mut found, with_writers(app, writers));
    }
    if wants(Harness::Opencode) {
        extend(
            &mut found,
            opencode::discover(opencode_urls, workspace.as_deref()),
        );
    }
    finish(found, workspace.as_deref())
}
