//! Claude transport against real fixtures: a fake `claude agents --json`, session metadata in
//! a fake HOME, and a Unix listener standing in for the messaging socket.
use harness_talk::model::NativePeer as Peer;
use harness_talk::{claude, error::Failure, model::*};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

static ENVIRONMENT: Mutex<()> = Mutex::new(());
const SESSION: &str = "3a1f7c52-6b8e-4d0f-9a21-5c4e8d7b6f30";
const MESSAGE: &str = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";

struct Fixture {
    dir: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let lock = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "htalk-claude-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join(".claude/sessions")).unwrap();
        fs::create_dir_all(dir.join("project")).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let mut fixture = Self {
            dir,
            saved: Vec::new(),
            _lock: lock,
        };
        for (key, value) in [
            ("HOME", fixture.dir.clone().into_os_string()),
            ("PATH", fixture.dir.join("bin").into_os_string()),
        ] {
            fixture.saved.push((key, std::env::var_os(key)));
            unsafe { std::env::set_var(key, value) };
        }
        let path = fixture.dir.join("bin/claude");
        fs::write(&path, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        fixture
    }
    fn workspace(&self) -> String {
        self.dir.join("project").to_string_lossy().into_owned()
    }
    fn socket(&self) -> PathBuf {
        self.dir.join("messaging.sock")
    }
    fn peer(&self) -> Peer {
        Peer {
            name: "receiver".into(),
            harness: Harness::Claude,
            session_id: SESSION.into(),
            workspace: self.workspace(),
            socket: None,
            url: None,
            retired_at: None,
        }
    }
    fn agents(&self, rows: &Value) {
        self.agents_text(&rows.to_string());
    }
    fn agents_text(&self, text: &str) {
        fs::write(self.dir.join("bin/agents.json"), text).unwrap();
    }
    fn metadata(&self, pid: i64, value: &Value) {
        fs::write(
            self.dir.join(format!(".claude/sessions/{pid}.json")),
            value.to_string(),
        )
        .unwrap();
    }
    /// One live session: agents row, matching metadata and an owned socket path.
    fn live(&self) {
        let cwd = self.workspace();
        self.agents(
            &json!([{"sessionId": "11111111-1111-4111-8111-111111111111", "cwd": cwd, "pid": 41},
            {"sessionId": SESSION, "cwd": cwd, "pid": 42, "status": "idle"}]),
        );
        self.metadata(
            42,
            &json!({"sessionId": SESSION, "cwd": cwd, "pid": 42,
            "messagingSocketPath": format!("{}//./messaging.sock", self.dir.display())}),
        );
    }
    fn notify(&self, body: &str) -> Outcome {
        claude::notify(&self.peer(), &message(), body, &|| Ok(None))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn message() -> harness_talk::model::Message {
    harness_talk::model::Message::from_row(
        Row {
            seq: 1,
            id: MESSAGE.into(),
            sender: "sender".into(),
            recipient: "receiver".into(),
            in_reply_to: None,
            body: "Question".into(),
            created_at: 1.0,
            ack_at: None,
            submission: Submission::SubmissionUnknown,
            notification_started_at: Some(1.5),
            notification_finished_at: None,
            notification_detail: None,
            wait_returned_at: None,
        },
        None,
    )
}

#[test]
fn discovery_failures_are_not_submitted_and_uncertain_writes_are_unknown() {
    let fixture = Fixture::new("failures");
    let cwd = fixture.workspace();
    let row = json!({"sessionId": SESSION, "cwd": cwd, "pid": 42});
    let good = json!({"sessionId": SESSION, "cwd": cwd, "messagingSocketPath": fixture.socket()});
    let cases: Vec<(Value, Option<Value>, Submission, &str)> = vec![
        (
            json!([]),
            None,
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!({}),
            None,
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!([row, row]),
            Some(good.clone()),
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!([{"sessionId": SESSION, "cwd": cwd, "pid": true}]),
            None,
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!([{"sessionId": SESSION, "cwd": cwd, "pid": 4.0}]),
            None,
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!([{"sessionId": SESSION, "cwd": "project", "pid": 42}]),
            Some(good.clone()),
            Submission::NotSubmitted,
            "recipient_unavailable",
        ),
        (
            json!([row]),
            None,
            Submission::NotSubmitted,
            "FileNotFoundError",
        ),
        (
            json!([row]),
            Some(
                json!({"sessionId": SESSION, "cwd": "/other", "messagingSocketPath": fixture.socket()}),
            ),
            Submission::NotSubmitted,
            "recipient_identity_changed",
        ),
        (
            json!([row]),
            Some(
                json!({"sessionId": "other", "cwd": cwd, "messagingSocketPath": fixture.socket()}),
            ),
            Submission::NotSubmitted,
            "recipient_identity_changed",
        ),
        (
            json!([row]),
            Some(json!({"sessionId": SESSION, "cwd": cwd})),
            Submission::NotSubmitted,
            "KeyError",
        ),
        (
            json!([row]),
            Some(json!({"sessionId": SESSION, "cwd": cwd, "messagingSocketPath": 7})),
            Submission::NotSubmitted,
            "TypeError",
        ),
        (
            json!([row]),
            Some(
                json!({"sessionId": SESSION, "cwd": cwd, "messagingSocketPath": fixture.dir.join("project")}),
            ),
            Submission::NotSubmitted,
            "recipient_socket_unavailable",
        ),
        (
            json!([row]),
            Some(
                json!({"sessionId": SESSION, "cwd": cwd, "messagingSocketPath": fixture.dir.join("absent.sock")}),
            ),
            Submission::NotSubmitted,
            "FileNotFoundError",
        ),
        (json!(5), None, Submission::NotSubmitted, "TypeError"),
        // Malformed discovery is still before any notification bytes are written.
        (
            json!([row, "text"]),
            None,
            Submission::NotSubmitted,
            "AttributeError",
        ),
        (
            json!({"sessionId": SESSION}),
            None,
            Submission::NotSubmitted,
            "AttributeError",
        ),
        (
            json!([row]),
            Some(json!(["list"])),
            Submission::NotSubmitted,
            "AttributeError",
        ),
    ];
    for (rows, metadata, submission, detail) in cases {
        fixture.agents(&rows);
        let _ = fs::remove_file(fixture.dir.join(".claude/sessions/42.json"));
        if let Some(metadata) = &metadata {
            fixture.metadata(42, metadata);
        }
        let outcome = fixture.notify("Notice");
        assert_eq!(
            (submission, detail),
            (outcome.submission, outcome.detail.as_str()),
            "{rows} {metadata:?}"
        );
    }
    fixture.agents_text("not json");
    assert_eq!("JSONDecodeError", fixture.notify("Notice").detail);
    fs::write(fixture.dir.join("bin/exit"), "3").unwrap();
    fixture.agents(&json!([]));
    assert_eq!("CalledProcessError", fixture.notify("Notice").detail);
    fs::remove_file(fixture.dir.join("bin/claude")).unwrap();
    let missing = fixture.notify("Notice");
    assert_eq!(
        (Submission::NotSubmitted, "FileNotFoundError"),
        (missing.submission, missing.detail.as_str())
    );

    drop(fixture);
    // A refused connection cannot have delivered a notification frame.
    let fixture = Fixture::new("refused");
    fixture.live();
    drop(UnixListener::bind(fixture.socket()).unwrap());
    let refused = fixture.notify("Notice");
    assert_eq!(
        (Submission::NotSubmitted, "ConnectionRefusedError"),
        (refused.submission, refused.detail.as_str())
    );

    // Once the write begins, even a broken connection leaves the outcome uncertain.
    fs::remove_file(fixture.socket()).unwrap();
    let listener = UnixListener::bind(fixture.socket()).unwrap();
    let broken = claude::notify(&fixture.peer(), &message(), "Notice", &|| {
        let (connection, _) = listener.accept().unwrap();
        connection.shutdown(std::net::Shutdown::Both).unwrap();
        Ok(None)
    });
    assert_eq!(Submission::SubmissionUnknown, broken.submission);
}

#[test]
fn symlinked_workspace_matches_only_its_registered_target() {
    let fixture = Fixture::new("symlink");
    let alias = fixture.dir.join("alias");
    std::os::unix::fs::symlink(fixture.dir.join("project"), &alias).unwrap();
    let row = json!({"sessionId": SESSION, "cwd": alias, "pid": 42});
    fixture.agents(&json!([row]));
    fixture.metadata(
        42,
        &json!({"sessionId": SESSION, "cwd": alias, "messagingSocketPath": fixture.socket()}),
    );
    let _listener = UnixListener::bind(fixture.socket()).unwrap();
    assert_eq!(
        fixture.socket(),
        claude::live_socket(&fixture.peer()).unwrap()
    );
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&fixture.dir, &alias).unwrap();
    assert_eq!(
        Failure::coded("recipient_unavailable"),
        claude::live_socket(&fixture.peer()).unwrap_err()
    );
}

const FAKE_CLAUDE: &str = r#"#!/usr/bin/python3 -B
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
assert sys.argv[1:] == ["agents", "--json"], sys.argv
sys.stdout.write(open(os.path.join(here, "agents.json")).read())
code = os.path.join(here, "exit")
sys.exit(int(open(code).read()) if os.path.exists(code) else 0)
"#;
