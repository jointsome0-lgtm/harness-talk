//! Claude transport against real fixtures: a fake `claude agents --json`, session metadata in
//! a fake HOME, and a Unix listener standing in for the messaging socket.
use harness_talk::{claude, error::Failure, model::*};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
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
        let db = rusqlite::Connection::open(fixture.state_db()).unwrap();
        db.execute_batch("CREATE TABLE messages (id TEXT, ack INTEGER, returned INTEGER)")
            .unwrap();
        db.execute("INSERT INTO messages VALUES (?1, 0, 0)", [MESSAGE])
            .unwrap();
        fixture
    }
    fn workspace(&self) -> String {
        self.dir.join("project").to_string_lossy().into_owned()
    }
    fn socket(&self) -> PathBuf {
        self.dir.join("messaging.sock")
    }
    fn state_db(&self) -> PathBuf {
        self.dir.join("messages.sqlite3")
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
    fn skip(&self) -> Result<Option<SkipReason>, Failure> {
        let db = rusqlite::Connection::open_with_flags(
            self.state_db(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| Failure::coded("notification_state_unavailable"))?;
        let (ack, returned): (i64, i64) = db
            .query_row(
                "SELECT ack, returned FROM messages WHERE id=?1",
                [MESSAGE],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| Failure::coded("notification_state_unavailable"))?;
        Ok(if ack != 0 {
            Some(SkipReason::AcknowledgedBeforeNotification)
        } else if returned != 0 {
            Some(SkipReason::ReturnedByRecipientWait)
        } else {
            None
        })
    }
    fn notify(&self, body: &str) -> Outcome {
        claude::notify(&self.peer(), &message(), body, &|| self.skip())
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

/// Accept one connection and return every byte written to it.
fn listen(path: &Path) -> JoinHandle<Vec<u8>> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    })
}

#[test]
fn frame_is_written_once_with_exact_keys_after_live_identity_checks() {
    let fixture = Fixture::new("frame");
    fixture.live();
    let server = listen(&fixture.socket());
    let outcome = fixture.notify("Check message é 🙂\nwith htalk");
    let bytes = server.join().unwrap();
    assert_eq!(
        (Submission::Submitted, "claude_socket_bytes_written"),
        (outcome.submission, outcome.detail.as_str())
    );
    assert!(
        bytes.is_ascii()
            && bytes.ends_with(b"}\n")
            && bytes.iter().filter(|b| **b == b'\n').count() == 1
    );
    let frame: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json!({"type": "user", "session_id": SESSION, "uuid": MESSAGE, "msg_id": MESSAGE, "from": "htalk:sender",
        "priority": "next", "message": {"role": "user", "content": "Check message é 🙂\nwith htalk"}}),
        frame
    );
    let keys: Vec<&str> = frame
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        vec![
            "type",
            "session_id",
            "uuid",
            "msg_id",
            "from",
            "priority",
            "message"
        ],
        keys
    );
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .starts_with(r#"{"type": "user", "session_id": ""#)
    );
    assert_eq!(
        json!({"harness": "claude", "session_id": SESSION, "workspace": fixture.workspace(),
        "socket": fixture.socket().to_str().unwrap()}),
        claude::probe(&fixture.peer()).unwrap()
    );
}

#[test]
fn acknowledgment_or_wait_return_during_preflight_writes_nothing() {
    for (column, reason) in [
        ("ack", "acknowledged_before_notification"),
        ("returned", "returned_by_recipient_wait"),
    ] {
        let fixture = Fixture::new("preflight");
        fixture.live();
        // The fake client marks the message while the adapter's discovery is running.
        fs::write(
            fixture.dir.join("bin/mark"),
            format!("{}\n{column}\n{MESSAGE}", fixture.state_db().display()),
        )
        .unwrap();
        let server = listen(&fixture.socket());
        let outcome = fixture.notify("Notice");
        assert_eq!(
            (Submission::NotSubmitted, reason),
            (outcome.submission, outcome.detail.as_str())
        );
        assert!(
            server.join().unwrap().is_empty(),
            "the connection was opened but no frame was written"
        );
    }
    let fixture = Fixture::new("state-unavailable");
    fixture.live();
    fs::remove_file(fixture.state_db()).unwrap();
    let server = listen(&fixture.socket());
    let outcome = fixture.notify("Notice");
    assert_eq!(
        (Submission::NotSubmitted, "notification_state_unavailable"),
        (outcome.submission, outcome.detail.as_str())
    );
    assert!(server.join().unwrap().is_empty());
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
        // Python let AttributeError escape the adapter, and the store recorded it as uncertain.
        (
            json!([row, "text"]),
            None,
            Submission::SubmissionUnknown,
            "AttributeError",
        ),
        (
            json!({"sessionId": SESSION}),
            None,
            Submission::SubmissionUnknown,
            "AttributeError",
        ),
        (
            json!([row]),
            Some(json!(["list"])),
            Submission::SubmissionUnknown,
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
    // After discovery succeeds, a refused connection is uncertain like any socket error.
    let fixture = Fixture::new("refused");
    fixture.live();
    drop(UnixListener::bind(fixture.socket()).unwrap());
    let refused = fixture.notify("Notice");
    assert_eq!(
        (Submission::SubmissionUnknown, "ConnectionRefusedError"),
        (refused.submission, refused.detail.as_str())
    );
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

#[test]
fn discovery_interfaces_return_raw_records_for_their_own_validation() {
    let fixture = Fixture::new("interfaces");
    fixture.live();
    let rows = claude::agents().unwrap();
    assert_eq!(2, rows.len());
    assert_eq!(json!(42), rows[1]["pid"]);
    assert_eq!(
        json!(SESSION),
        claude::session_metadata(42).unwrap()["sessionId"]
    );
    assert_eq!(
        Failure::Class("FileNotFoundError"),
        claude::session_metadata(7).unwrap_err()
    );
    fixture.agents(&json!({"rows": []}));
    let invalid = claude::agents().unwrap_err();
    assert_eq!(
        ("invalid_claude_agents_response", "ValueError"),
        (invalid.to_string().as_str(), invalid.class_name())
    );
    fixture.agents_text("[");
    assert_eq!(
        Failure::Class("JSONDecodeError"),
        claude::agents().unwrap_err()
    );
}

const FAKE_CLAUDE: &str = r#"#!/usr/bin/python3 -B
import os, sqlite3, sys
here = os.path.dirname(os.path.abspath(__file__))
assert sys.argv[1:] == ["agents", "--json"], sys.argv
mark = os.path.join(here, "mark")
if os.path.exists(mark):
    path, column, ident = open(mark).read().split("\n")
    with sqlite3.connect(path) as db:
        db.execute(f"UPDATE messages SET {column}=1 WHERE id=?", (ident,))
sys.stdout.write(open(os.path.join(here, "agents.json")).read())
code = os.path.join(here, "exit")
sys.exit(int(open(code).read()) if os.path.exists(code) else 0)
"#;
