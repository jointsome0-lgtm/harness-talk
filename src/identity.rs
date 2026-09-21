//! Recognize the Claude Code session whose tool command runs htalk.
//!
//! The result only selects a default peer name. Every piece of evidence is readable
//! and forgeable by the same OS account, so it is not authentication.
use crate::{model::NativeSession, os, validate};
use serde_json::Value;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub const MAX_DEPTH: usize = 64;
const CLIENTS: [&[u8]; 3] = [b"claude", b"codex", b"opencode"];

/// Any failure reading or parsing a /proc entry; each call site maps it to its own reason.
struct Unreadable;

/// Return comm, parent PID and start time from /proc/PID/stat.
fn process(proc_root: &Path, pid: i64) -> Result<(String, i64, String), Unreadable> {
    let bytes =
        std::fs::read(proc_root.join(pid.to_string()).join("stat")).map_err(|_| Unreadable)?;
    let text = String::from_utf8(bytes).map_err(|_| Unreadable)?;
    // comm may contain spaces and parentheses; the fields after the last ")" cannot.
    let (head, tail) = text.rsplit_once(')').unwrap_or(("", &text));
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let parent = fields
        .get(1)
        .ok_or(Unreadable)?
        .parse::<i64>()
        .map_err(|_| Unreadable)?;
    let started = fields.get(19).ok_or(Unreadable)?.to_string();
    let comm = head.split_once('(').map_or("", |(_, comm)| comm).to_owned();
    Ok((comm, parent, started))
}

fn looks_like_client(proc_root: &Path, pid: i64, comm: &str) -> Result<bool, Unreadable> {
    // The kernel truncates comm to 15 bytes, so compare prefixes of comm and the executable name.
    let exe =
        std::fs::read_link(proc_root.join(pid.to_string()).join("exe")).map_err(|_| Unreadable)?;
    let exe = exe.as_os_str().as_bytes();
    let name = exe.rsplit(|b| *b == b'/').next().unwrap_or(exe);
    Ok(CLIENTS
        .iter()
        .any(|client| comm.as_bytes().starts_with(client) || name.starts_with(client)))
}

fn unrecognized(reason: &'static str) -> NativeSession {
    NativeSession::Unrecognized { reason }
}

/// Return the recognized session_id and workspace, or the first failed condition as reason.
/// `sessions_dir` defaults to `~/.claude/sessions` and `pid` to this process.
pub fn claude_session(
    env: &dyn Fn(&str) -> Option<String>,
    proc_root: &Path,
    sessions_dir: Option<&Path>,
    pid: Option<u32>,
) -> NativeSession {
    let present = |name| env(name).filter(|value| !value.is_empty());
    let (Some(session_id), Some(claude_pid)) =
        (present("CLAUDE_CODE_SESSION_ID"), present("CLAUDE_PID"))
    else {
        return unrecognized("claude_session_variables_absent");
    };
    let canonical = validate::uuid(&session_id).is_ok_and(|id| id == session_id);
    if !canonical || !claude_pid.bytes().all(|b| b.is_ascii_digit()) {
        return unrecognized("claude_session_variables_invalid");
    }
    // An all-digit value beyond any PID cannot name a process entry.
    let claude_pid = claude_pid.parse::<i64>().ok();
    // A nested Codex command inherits Claude's variables; see the documented tmux limitation.
    if present("CODEX_THREAD_ID").is_some() {
        return unrecognized("codex_thread_id_present");
    }
    let Some((claude_pid, started)) =
        claude_pid.and_then(|pid| Some((pid, process(proc_root, pid).ok()?.2)))
    else {
        return unrecognized("claude_process_unavailable");
    };
    let sessions =
        sessions_dir.map_or_else(|| os::home().join(".claude/sessions"), Path::to_path_buf);
    let metadata = std::fs::read(sessions.join(format!("{claude_pid}.json")))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let Some(metadata) = metadata else {
        return unrecognized("claude_session_metadata_unavailable");
    };
    let Some(metadata) = metadata.as_object() else {
        return unrecognized("claude_session_metadata_mismatch");
    };
    let start = match metadata.get("procStart") {
        Some(Value::String(start)) => Some(start.clone()),
        Some(Value::Number(start)) if start.is_i64() || start.is_u64() => Some(start.to_string()),
        _ => None,
    };
    if metadata.get("sessionId").and_then(Value::as_str) != Some(session_id.as_str())
        || metadata.get("pid").and_then(Value::as_i64) != Some(claude_pid)
        || start.as_deref() != Some(started.as_str())
    {
        return unrecognized("claude_session_metadata_mismatch");
    }
    match ancestry(proc_root, claude_pid, pid.unwrap_or_else(std::process::id)) {
        Ok(Some(reason)) => return unrecognized(reason),
        Ok(None) => {}
        Err(Unreadable) => return unrecognized("process_ancestry_unavailable"),
    }
    let workspace = metadata
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_owned);
    NativeSession::Recognized {
        session_id,
        workspace,
    }
}

fn ancestry(
    proc_root: &Path,
    claude_pid: i64,
    pid: u32,
) -> Result<Option<&'static str>, Unreadable> {
    let mut between = Vec::new();
    let mut parent = process(proc_root, i64::from(pid))?.1;
    while parent != claude_pid {
        if parent <= 0 || between.len() == MAX_DEPTH {
            return Ok(Some("claude_pid_not_an_ancestor"));
        }
        let (comm, grandparent, _) = process(proc_root, parent)?;
        between.push((parent, comm));
        parent = grandparent;
    }
    // Executables are inspected only below Claude, where they share its owner.
    for (pid, comm) in &between {
        if looks_like_client(proc_root, *pid, comm)? {
            return Ok(Some("nested_client_process"));
        }
    }
    Ok(None)
}
