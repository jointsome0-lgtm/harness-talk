//! Native Claude recognition against a synthetic /proc tree and sessions directory,
//! never the running system's.
use harness_talk::identity::claude_session;
use harness_talk::model::NativeSession;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

/// A private temporary directory removed on drop (std only, like tempfile::TempDir).
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let path = std::env::temp_dir().join(format!(
            "htalk-identity-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SESSION: &str = "4b1f7f0e-2f4c-4a5e-9d1b-6f0c2d9e8a71";

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    proc: PathBuf,
    sessions: PathBuf,
    env: HashMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new();
        let root = dir.0.clone();
        let (proc, sessions) = (root.join("proc"), root.join("sessions"));
        fs::create_dir(&sessions).unwrap();
        let env = HashMap::from([
            ("CLAUDE_CODE_SESSION_ID".into(), SESSION.into()),
            ("CLAUDE_PID".into(), "20".into()),
        ]);
        let f = Self {
            _dir: dir,
            root,
            proc,
            sessions,
            env,
        };
        // htalk (40) runs in a tool shell (30) of Claude Code (20), started from a terminal shell (10).
        f.process(1, 0, "systemd", 100, "/usr/lib/systemd/systemd");
        f.process(10, 1, "bash", 100, "/usr/bin/bash");
        f.process(
            20,
            10,
            "claude",
            4242,
            "/home/user/.local/share/claude/versions/2.1.274",
        );
        f.process(30, 20, "bash", 100, "/usr/bin/bash");
        f.process(40, 30, "htalk", 100, "/usr/bin/python3.14");
        f.metadata(json!({}));
        f
    }

    fn process(&self, pid: u32, parent: u32, comm: &str, start: u64, exe: &str) {
        let dir = self.proc.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        let mut fields = vec!["S".to_string(), parent.to_string()];
        fields.extend(std::iter::repeat_n("0".to_string(), 17));
        fields.push(start.to_string());
        fields.extend(std::iter::repeat_n("0".to_string(), 32));
        fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) {}\n", fields.join(" ")),
        )
        .unwrap();
        symlink(exe, dir.join("exe")).unwrap();
    }

    fn shell(&self, pid: u32, parent: u32, comm: &str) {
        self.process(pid, parent, comm, 100, "/usr/bin/bash")
    }

    fn metadata(&self, changes: Value) {
        let mut saved = json!({"pid": 20, "sessionId": SESSION, "procStart": "4242",
            "cwd": self.root.to_str().unwrap(), "messagingSocketPath": "/unused.sock"});
        for (key, value) in changes.as_object().unwrap() {
            saved[key] = value.clone();
        }
        fs::write(self.sessions.join("20.json"), saved.to_string()).unwrap();
    }

    fn detect_with(&self, pid: u32, extra: &[(&str, &str)], proc: &Path) -> NativeSession {
        let mut env = self.env.clone();
        for (key, value) in extra {
            env.insert(key.to_string(), value.to_string());
        }
        claude_session(
            &|name| env.get(name).cloned(),
            proc,
            Some(&self.sessions),
            Some(pid),
        )
    }
    fn detect(&self, pid: u32, extra: &[(&str, &str)]) -> NativeSession {
        self.detect_with(pid, extra, &self.proc)
    }
    fn reason(&self, pid: u32, extra: &[(&str, &str)]) -> &'static str {
        match self.detect(pid, extra) {
            NativeSession::Unrecognized { reason } => reason,
            other => panic!("{other:?}"),
        }
    }
}

fn recognized(f: &Fixture) -> NativeSession {
    NativeSession::Recognized {
        session_id: SESSION.into(),
        workspace: Some(f.root.to_str().unwrap().into()),
    }
}

#[test]
fn refuses_commands_of_clients_nested_in_claude() {
    let f = Fixture::new();
    assert_eq!(
        "codex_thread_id_present",
        f.reason(
            40,
            &[("CODEX_THREAD_ID", "0c4a1c0e-3a55-4c1f-a8f5-7d3b1f7e2a10")]
        )
    );
    // An empty variable is absent.
    assert_eq!(recognized(&f), f.detect(40, &[("CODEX_THREAD_ID", "")]));
    // The kernel truncates comm to 15 bytes, and a client may run under another process name.
    let clients = [
        ("codex-code-mode", "/usr/bin/node"),
        ("MainThread", "/opt/codex/codex-x86_64-unknown-linux-musl"),
        ("opencode", "/usr/bin/bun"),
        ("claude", "/home/user/.local/share/claude/versions/2.1.274"),
    ];
    for (pid, (comm, exe)) in (300..).step_by(10).zip(clients) {
        f.process(pid, 30, comm, 100, exe);
        f.shell(pid + 1, pid, "bash");
        f.shell(pid + 2, pid + 1, "htalk");
        assert_eq!(
            "nested_client_process",
            f.reason(pid + 2, &[]),
            "{comm} {exe}"
        );
    }
}

#[test]
fn every_intermediate_executable_is_read_until_a_client_is_found() {
    let f = Fixture::new();
    // A client comm does not excuse its own unreadable executable.
    f.process(90, 30, "codex", 100, "/opt/codex/codex");
    f.shell(91, 90, "htalk");
    fs::remove_file(f.proc.join("90/exe")).unwrap();
    assert_eq!("process_ancestry_unavailable", f.reason(91, &[]));
    // A malformed stat entry between htalk and Claude is unavailable evidence, not a refusal.
    fs::write(f.proc.join("30/stat"), "30 (bash) S\n").unwrap();
    assert_eq!("process_ancestry_unavailable", f.reason(40, &[]));
}
