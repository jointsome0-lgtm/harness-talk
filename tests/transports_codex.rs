//! Codex transports against real fixtures: a WebSocket app-server on a Unix socket, a fake
//! `codex` executable for `queue` and `app-server --stdio`, and fixture SQLite state.
use harness_talk::model::NativePeer as Peer;
use harness_talk::{
    codex::{self, rpc::Rpc, state},
    error::Failure,
    model::*,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tungstenite::Message as Frame;

static ENVIRONMENT: Mutex<()> = Mutex::new(());
const QUEUE_ID: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";

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
        let dir =
            std::env::temp_dir().join(format!("htalk-codex-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let mut fixture = Self {
            dir,
            saved: Vec::new(),
            _lock: lock,
        };
        let bin = fixture.dir.join("bin");
        fixture.set("CODEX_HOME", fixture.dir.clone().into_os_string());
        fixture.set("CODEX_SQLITE_HOME", "".into());
        fixture.set("HOME", fixture.dir.clone().into_os_string());
        fixture.set("PATH", bin.into_os_string());
        fixture
    }
    fn set(&mut self, key: &'static str, value: OsString) {
        self.saved.push((key, std::env::var_os(key)));
        unsafe { std::env::set_var(key, value) };
    }
    fn workspace(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }
    fn peer(&self, socket: Option<&Path>) -> Peer {
        Peer {
            name: "reader".into(),
            harness: Harness::Codex,
            session_id: "7d3d4bd5-9d5f-4f8f-8c1c-0e4a1f6b2a10".into(),
            workspace: self.workspace(),
            socket: socket.map(|s| s.to_string_lossy().into_owned()),
            url: None,
            retired_at: None,
        }
    }
    fn thread(&self, cwd: &str, archived: i64, source: &str) {
        let db = rusqlite::Connection::open(self.dir.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, cwd TEXT, archived INTEGER, source TEXT); DELETE FROM threads").unwrap();
        db.execute(
            "INSERT INTO threads VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![self.peer(None).session_id, cwd, archived, source],
        )
        .unwrap();
    }
    fn mode(&self, mode: &str) {
        fs::write(self.dir.join("bin/mode"), mode).unwrap();
    }
    fn fake_codex(&self) {
        let path = self.dir.join("bin/codex");
        fs::write(&path, FAKE_CODEX).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn calls(&self) -> Vec<Value> {
        lines(&self.dir.join("bin/calls.jsonl"))
    }

    /// A message store stand-in: the transports' final check reads this SQLite row.
    fn state_db(&self) -> PathBuf {
        let path = self.dir.join("messages.sqlite3");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (id TEXT, ack INTEGER, returned INTEGER)",
        )
        .unwrap();
        path
    }
    fn message(&self, id: &str) -> PathBuf {
        let path = self.state_db();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO messages VALUES (?1, 0, 0)", [id])
            .unwrap();
        path
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

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn mark(db: &Path, id: &str, column: &str) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(&format!("UPDATE messages SET {column}=1 WHERE id=?1"), [id])
        .unwrap();
}

fn read_skip(db: &Path, id: &str) -> Result<Option<SkipReason>, Failure> {
    let db = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| Failure::coded("notification_state_unavailable"))?;
    let (ack, returned): (i64, i64) = db
        .query_row(
            "SELECT ack, returned FROM messages WHERE id=?1",
            [id],
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

fn message(
    peer: &Peer,
    detail: Option<&str>,
    submission: Submission,
    finished: bool,
) -> harness_talk::model::Message {
    harness_talk::model::Message::from_row(
        Row {
            seq: 1,
            id: "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11".into(),
            sender: "sender".into(),
            recipient: peer.name.clone(),
            in_reply_to: None,
            body: "Question".into(),
            created_at: 1.0,
            ack_at: Some(2.0),
            submission,
            notification_started_at: Some(1.5),
            notification_finished_at: finished.then_some(1.6),
            notification_detail: detail.map(str::to_owned),
            wait_returned_at: None,
        },
        None,
    )
}

type Handler = Box<dyn FnMut(&Value) -> Option<Value> + Send>;

/// One WebSocket connection on a Unix socket. Returns every frame the client sent.
fn serve(path: &Path, mut handler: Handler) -> JoinHandle<Vec<Value>> {
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let mut seen = Vec::new();
        loop {
            let text = match socket.read() {
                Ok(Frame::Text(text)) => text,
                Ok(Frame::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            let frame: Value = serde_json::from_str(text.as_str()).unwrap();
            seen.push(frame.clone());
            if frame.get("id").is_none() {
                continue;
            }
            // Unrelated traffic before each answer must be skipped by the client.
            socket
                .send(Frame::text(
                    json!({"method": "thread/status/changed", "params": {}}).to_string(),
                ))
                .unwrap();
            socket
                .send(Frame::text(
                    json!({"id": 9999, "result": {"thread": "wrong"}}).to_string(),
                ))
                .unwrap();
            socket.send(Frame::Ping(Vec::new().into())).unwrap();
            match handler(&frame) {
                Some(reply) => socket.send(Frame::text(reply.to_string())).unwrap(),
                None => break,
            }
        }
        seen
    })
}

/// A well-behaved app-server for `peer`; `on_read` runs while the client's preflight is in progress.
fn app_server(
    peer: &Peer,
    mut on_read: impl FnMut() + Send + 'static,
    add: Option<Value>,
) -> Handler {
    let (session, workspace) = (peer.session_id.clone(), peer.workspace.clone());
    Box::new(move |frame| {
        let id = frame["id"].clone();
        let result = match frame["method"].as_str().unwrap() {
            "initialize" => {
                assert_eq!(
                    json!(true),
                    frame["params"]["capabilities"]["experimentalApi"]
                );
                json!({})
            }
            "thread/read" => {
                on_read();
                json!({"thread": {"id": session, "cwd": workspace, "status": {"type": "idle"}}})
            }
            "thread/queue/add" => add.clone().unwrap_or_else(|| {
                json!({"queuedSubmission": {"id": "queue-receipt",
                "clientUserMessageId": frame["params"]["clientUserMessageId"]}})
            }),
            "thread/queue/delete" => json!({"deleted": true}),
            other => panic!("unexpected method {other}"),
        };
        Some(json!({"id": id, "result": result}))
    })
}

fn methods(frames: &[Value]) -> Vec<&str> {
    frames
        .iter()
        .map(|f| f["method"].as_str().unwrap())
        .collect()
}

#[test]
fn socket_queue_add_uses_exact_identity_and_saves_the_receipt() {
    let fixture = Fixture::new("socket-add");
    let path = fixture.dir.join("server.sock");
    let peer = fixture.peer(Some(&path));
    let id = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";
    let db = fixture.message(id);
    let server = serve(&path, app_server(&peer, || (), None));
    let outcome = codex::notify(&peer, id, "Notice é", &|| read_skip(&db, id));
    let frames = server.join().unwrap();
    assert_eq!(
        (Submission::Submitted, "codex_queued:queue-receipt"),
        (outcome.submission, outcome.detail.as_str())
    );
    assert_eq!(
        vec![
            "initialize",
            "initialized",
            "thread/read",
            "thread/queue/add"
        ],
        methods(&frames)
    );
    assert_eq!(
        json!({"threadId": peer.session_id, "includeTurns": false}),
        frames[2]["params"]
    );
    assert_eq!(
        json!({"threadId": peer.session_id, "clientUserMessageId": id, "input": [{"type": "text", "text": "Notice é"}]}),
        frames[3]["params"]
    );
    assert_eq!("harness-talk", frames[0]["params"]["clientInfo"]["name"]);
}

#[test]
fn socket_acknowledgment_or_wait_return_during_preflight_prevents_queue_add() {
    for (column, reason) in [
        ("ack", "acknowledged_before_notification"),
        ("returned", "returned_by_recipient_wait"),
    ] {
        let fixture = Fixture::new("socket-preflight");
        let path = fixture.dir.join("server.sock");
        let peer = fixture.peer(Some(&path));
        let id = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";
        let db = fixture.message(id);
        let marked = db.clone();
        let server = serve(
            &path,
            app_server(&peer, move || mark(&marked, id, column), None),
        );
        let outcome = codex::notify(&peer, id, "Notice", &|| read_skip(&db, id));
        let frames = server.join().unwrap();
        assert_eq!(
            (Submission::NotSubmitted, reason),
            (outcome.submission, outcome.detail.as_str())
        );
        assert_eq!(
            vec!["initialize", "initialized", "thread/read"],
            methods(&frames)
        );
    }
}

#[test]
fn socket_failures_before_and_after_the_queue_attempt_are_distinct() {
    let id = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";
    type Case = (
        &'static str,
        Box<dyn Fn(&Peer) -> Handler>,
        Submission,
        &'static str,
    );
    let cases: Vec<Case> = vec![
        (
            "mismatch",
            Box::new(|p| {
                app_server(
                    p,
                    || (),
                    Some(json!({"queuedSubmission": {"id": "q", "clientUserMessageId": "other"}})),
                )
            }),
            Submission::SubmissionUnknown,
            "codex_queue_receipt_mismatch",
        ),
        (
            "missing-receipt",
            Box::new(|p| app_server(p, || (), Some(json!({})))),
            Submission::SubmissionUnknown,
            "KeyError",
        ),
        (
            "rejected-add",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                Box::new(move |f| {
                    if f["method"] == "thread/queue/add" {
                        Some(
                            json!({"id": f["id"], "error": {"code": -32600, "message": "/private/path secret_token"}}),
                        )
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::SubmissionUnknown,
            "codex_rpc_rejected:-32600",
        ),
        (
            "closed-after-add",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                Box::new(move |f| {
                    if f["method"] == "thread/queue/add" {
                        None
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::SubmissionUnknown,
            "codex_websocket_failure",
        ),
        (
            "changed",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                Box::new(move |f| {
                    if f["method"] == "thread/read" {
                        Some(
                            json!({"id": f["id"], "result": {"thread": {"id": f["params"]["threadId"], "cwd": "/other", "status": {"type": "idle"}}}}),
                        )
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::NotSubmitted,
            "recipient_identity_changed",
        ),
        (
            "not-loaded",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                let ws = p.workspace.clone();
                Box::new(move |f| {
                    if f["method"] == "thread/read" {
                        Some(
                            json!({"id": f["id"], "result": {"thread": {"id": f["params"]["threadId"], "cwd": ws, "status": {"type": "notLoaded"}}}}),
                        )
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::NotSubmitted,
            "recipient_not_loaded",
        ),
        (
            "closed-in-preflight",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                Box::new(move |f| {
                    if f["method"] == "thread/read" {
                        None
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::NotSubmitted,
            "codex_websocket_failure",
        ),
        // Invalid preflight metadata is rejected before thread/queue/add.
        (
            "non-object-thread",
            Box::new(|p| {
                let mut inner = app_server(p, || (), None);
                Box::new(move |f| {
                    if f["method"] == "thread/read" {
                        Some(json!({"id": f["id"], "result": {"thread": "text"}}))
                    } else {
                        inner(f)
                    }
                })
            }),
            Submission::NotSubmitted,
            "AttributeError",
        ),
    ];
    for (name, handler, submission, detail) in cases {
        let fixture = Fixture::new(name);
        let path = fixture.dir.join("server.sock");
        let peer = fixture.peer(Some(&path));
        let db = fixture.message(id);
        let server = serve(&path, handler(&peer));
        let outcome = codex::notify(&peer, id, "Notice", &|| read_skip(&db, id));
        let frames = server.join().unwrap();
        assert_eq!(
            submission == Submission::SubmissionUnknown,
            methods(&frames).contains(&"thread/queue/add"),
            "{name}"
        );
        assert_eq!(
            (submission, detail),
            (outcome.submission, outcome.detail.as_str()),
            "{name}"
        );
    }
    let fixture = Fixture::new("no-socket");
    let id_db = fixture.message(id);
    let absent = codex::notify(
        &fixture.peer(Some(&fixture.dir.join("absent.sock"))),
        id,
        "Notice",
        &|| read_skip(&id_db, id),
    );
    assert_eq!(
        (Submission::NotSubmitted, "FileNotFoundError"),
        (absent.submission, absent.detail.as_str())
    );
    fs::write(fixture.dir.join("file.sock"), "").unwrap();
    let regular = codex::notify(
        &fixture.peer(Some(&fixture.dir.join("file.sock"))),
        id,
        "Notice",
        &|| read_skip(&id_db, id),
    );
    assert_eq!(
        (Submission::NotSubmitted, "recipient_socket_unavailable"),
        (regular.submission, regular.detail.as_str())
    );
    drop(UnixListener::bind(fixture.dir.join("stale.sock")).unwrap());
    let refused = codex::notify(
        &fixture.peer(Some(&fixture.dir.join("stale.sock"))),
        id,
        "Notice",
        &|| read_skip(&id_db, id),
    );
    assert_eq!(
        (Submission::NotSubmitted, "ConnectionRefusedError"),
        (refused.submission, refused.detail.as_str())
    );
    assert!(
        fixture.calls().is_empty(),
        "an explicit socket never falls back to the native CLI"
    );
}

#[test]
fn socket_probe_and_cleanup_repeat_the_live_identity_check() {
    let fixture = Fixture::new("socket-cleanup");
    fixture.fake_codex();
    let path = fixture.dir.join("server.sock");
    let peer = fixture.peer(Some(&path));
    let server = serve(&path, app_server(&peer, || (), None));
    assert_eq!(
        json!({"harness": "codex", "session_id": peer.session_id, "workspace": peer.workspace, "status": "idle"}),
        codex::probe(&peer).unwrap()
    );
    server.join().unwrap();

    let saved = message(
        &peer,
        Some(&format!("codex_queued:{}", QUEUE_ID.to_uppercase())),
        Submission::Submitted,
        true,
    );
    let server = serve(&path, app_server(&peer, || (), None));
    let cleanup = codex::dismiss(&peer, &saved);
    let frames = server.join().unwrap();
    assert_eq!(
        (CleanupStatus::Removed, Some(QUEUE_ID)),
        (cleanup.status, cleanup.queue_id.as_deref())
    );
    assert_eq!(None, cleanup.detail);
    assert_eq!(
        vec![
            "initialize",
            "initialized",
            "thread/read",
            "thread/queue/delete"
        ],
        methods(&frames)
    );
    assert_eq!(
        json!({"threadId": peer.session_id, "queuedSubmissionId": QUEUE_ID}),
        frames[3]["params"]
    );

    for (thread, detail) in [
        (
            json!({"id": peer.session_id, "cwd": "/other", "status": {"type": "idle"}}),
            "recipient_identity_changed",
        ),
        (json!([]), "AttributeError"),
    ] {
        let server = serve(
            &path,
            Box::new(move |f| {
                Some(match f["method"].as_str().unwrap() {
                    "initialize" => json!({"id": f["id"], "result": {}}),
                    _ => json!({"id": f["id"], "result": {"thread": thread}}),
                })
            }),
        );
        let cleanup = codex::dismiss(&peer, &saved);
        let frames = server.join().unwrap();
        assert_eq!(
            (CleanupStatus::Unavailable, Some(detail)),
            (cleanup.status, cleanup.detail.as_deref())
        );
        assert!(!methods(&frames).contains(&"thread/queue/delete"));
    }
    assert!(fixture.calls().is_empty(), "no native fallback");
}

#[test]
fn native_queue_runs_once_with_the_exact_uuid_and_checks_the_receipt() {
    let fixture = Fixture::new("cli-queue");
    fixture.fake_codex();
    fixture.thread(&fixture.workspace(), 0, "cli");
    let peer = fixture.peer(None);
    let id = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";
    let db = fixture.message(id);
    let body = "[harness-talk peer notification]\nline 'two' $x";
    let cases = [
        (
            "ok",
            Submission::Submitted,
            format!("codex_cli_queued:{QUEUE_ID}"),
        ),
        (
            "crlf",
            Submission::Submitted,
            format!("codex_cli_queued:{QUEUE_ID}"),
        ),
        (
            "fail",
            Submission::SubmissionUnknown,
            "codex_cli_unconfirmed_receipt".into(),
        ),
        (
            "garbage",
            Submission::SubmissionUnknown,
            "codex_cli_unconfirmed_receipt".into(),
        ),
        (
            "other_thread",
            Submission::SubmissionUnknown,
            "codex_cli_unconfirmed_receipt".into(),
        ),
        (
            "uppercase",
            Submission::SubmissionUnknown,
            "codex_cli_unconfirmed_receipt".into(),
        ),
        (
            "hyphens",
            Submission::SubmissionUnknown,
            "codex_cli_invalid_queue_id".into(),
        ),
        (
            "latin1",
            Submission::SubmissionUnknown,
            "UnicodeDecodeError".into(),
        ),
    ];
    let count = cases.len();
    for (mode, submission, detail) in cases {
        fixture.mode(mode);
        let outcome = codex::notify(&peer, id, body, &|| read_skip(&db, id));
        assert_eq!(
            (submission, detail.as_str()),
            (outcome.submission, outcome.detail.as_str()),
            "{mode}"
        );
    }
    let calls = fixture.calls();
    assert_eq!(count, calls.len());
    assert_eq!(
        json!(["queue", "--thread", peer.session_id, "--message", body]),
        calls[0]
    );
}

#[test]
fn native_queue_is_not_run_without_the_saved_identity_or_after_a_final_check_receipt() {
    let fixture = Fixture::new("cli-preflight");
    fixture.fake_codex();
    fixture.mode("ok");
    let peer = fixture.peer(None);
    let id = "5b0c8d4e-5f55-4a51-9d0c-2f5d4b8a7c11";
    let db = fixture.message(id);
    let notify = || codex::notify(&peer, id, "Notice", &|| read_skip(&db, id));
    let not_submitted = |detail: &str| {
        let o = notify();
        assert_eq!(
            (Submission::NotSubmitted, detail),
            (o.submission, o.detail.as_str())
        );
    };
    not_submitted("OperationalError");
    assert!(
        !fixture.dir.join("state_5.sqlite").exists(),
        "the saved state is never created"
    );
    for (cwd, archived, source, detail) in [
        ("/other", 0, "cli", "recipient_identity_changed"),
        (
            fixture.workspace().as_str(),
            1,
            "cli",
            "recipient_is_not_an_unarchived_codex_cli_session",
        ),
        (
            fixture.workspace().as_str(),
            0,
            "exec",
            "recipient_is_not_an_unarchived_codex_cli_session",
        ),
    ] {
        fixture.thread(cwd, archived, source);
        not_submitted(detail);
    }
    rusqlite::Connection::open(fixture.dir.join("state_5.sqlite"))
        .unwrap()
        .execute("DELETE FROM threads", [])
        .unwrap();
    not_submitted("recipient_not_in_codex_state");
    fixture.thread(&fixture.workspace(), 0, "cli");
    // The identity check precedes the final state check, which precedes the one queue command.
    mark(&db, id, "returned");
    not_submitted("returned_by_recipient_wait");
    mark(&db, id, "ack");
    not_submitted("acknowledged_before_notification");
    let unavailable = codex::notify(&peer, id, "Notice", &|| {
        read_skip(&fixture.dir.join("missing.sqlite3"), id)
    });
    assert_eq!(
        (Submission::NotSubmitted, "notification_state_unavailable"),
        (unavailable.submission, unavailable.detail.as_str())
    );
    assert!(fixture.calls().is_empty());
    fs::remove_file(fixture.dir.join("bin/codex")).unwrap();
    let fresh = fixture.message("11111111-2222-4333-8444-555555555555");
    let missing = codex::notify(
        &peer,
        "11111111-2222-4333-8444-555555555555",
        "Notice",
        &|| read_skip(&fresh, "11111111-2222-4333-8444-555555555555"),
    );
    assert_eq!(
        (Submission::NotSubmitted, "FileNotFoundError"),
        (missing.submission, missing.detail.as_str())
    );
}

#[test]
fn saved_identity_follows_configuration_precedence() {
    let mut fixture = Fixture::new("config");
    let peer = fixture.peer(None);
    let home = fixture.dir.clone();
    assert_eq!(home, state::codex_home());
    assert_eq!(home.join("state_5.sqlite"), state::state_path().unwrap());
    fs::create_dir_all(home.join("env")).unwrap();
    fixture.set(
        "CODEX_SQLITE_HOME",
        format!("  {}\n", home.join("env").display()).into(),
    );
    assert_eq!(
        home.join("env/state_5.sqlite"),
        state::state_path().unwrap()
    );
    fs::write(home.join("config.toml"), "sqlite_home = \"relative\"\n").unwrap();
    assert_eq!(
        home.join("relative/state_5.sqlite"),
        state::state_path().unwrap()
    );
    let configured = home.join("configured");
    fs::create_dir_all(&configured).unwrap();
    fs::write(
        home.join("config.toml"),
        format!("sqlite_home = {:?}\n", configured.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        configured.join("state_5.sqlite"),
        state::state_path().unwrap()
    );
    fs::write(home.join("config.toml"), "sqlite_home = \"\"\n").unwrap();
    assert_eq!(
        home.join("env/state_5.sqlite"),
        state::state_path().unwrap()
    );
    fs::write(home.join("config.toml"), "sqlite_home = 5\n").unwrap();
    assert_eq!(
        Failure::Class("TypeError"),
        state::state_path().unwrap_err()
    );
    fs::write(home.join("config.toml"), "sqlite_home = [\n").unwrap();
    assert_eq!(
        Failure::Class("TOMLDecodeError"),
        state::state_path().unwrap_err()
    );

    fs::write(
        home.join("config.toml"),
        format!("sqlite_home = {:?}\n", configured.to_str().unwrap()),
    )
    .unwrap();
    let db = rusqlite::Connection::open(configured.join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE threads (id TEXT, cwd TEXT, archived INTEGER, source TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO threads VALUES (?1, ?2, 0, 'cli')",
        [&peer.session_id, &peer.workspace],
    )
    .unwrap();
    assert_eq!(
        Some((
            peer.session_id.clone(),
            json!(peer.workspace),
            0,
            json!("cli")
        )),
        state::saved_thread(&peer.session_id).unwrap()
    );
    assert_eq!(
        None,
        state::saved_thread("00000000-0000-4000-8000-000000000000").unwrap()
    );
    assert_eq!(
        json!({"harness": "codex", "session_id": peer.session_id, "workspace": peer.workspace,
        "metadata_source": configured.join("state_5.sqlite").to_str().unwrap(), "transport": "codex_cli_queue",
        "runtime_status": "unknown"}),
        codex::probe(&peer).unwrap()
    );
    // A symlinked saved workspace matches only while it points at the registered target.
    let alias = home.join("alias");
    std::os::unix::fs::symlink(&home, &alias).unwrap();
    db.execute("UPDATE threads SET cwd=?1", [alias.to_str().unwrap()])
        .unwrap();
    assert_eq!(peer.session_id, codex::probe(&peer).unwrap()["session_id"]);
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&configured, &alias).unwrap();
    assert_eq!(
        Failure::coded("recipient_identity_changed"),
        codex::probe(&peer).unwrap_err()
    );
}

#[test]
fn cleanup_requires_an_acknowledged_confirmed_receipt_and_the_saved_identity() {
    let fixture = Fixture::new("cleanup-rules");
    fixture.fake_codex();
    fixture.mode("removed");
    fixture.thread("/other-workspace", 0, "cli");
    let peer = fixture.peer(None);
    let receipt = format!("codex_cli_queued:{QUEUE_ID}");
    let mut unacknowledged = message(&peer, Some(&receipt), Submission::Submitted, true);
    unacknowledged.row.ack_at = None;
    let mut other = message(&peer, Some(&receipt), Submission::Submitted, true);
    other.row.recipient = "other".into();
    let mut claude = peer.clone();
    claude.harness = Harness::Claude;
    let cases = [
        (
            &peer,
            unacknowledged,
            CleanupStatus::Skipped,
            "message_not_acknowledged_by_recipient",
        ),
        (
            &peer,
            other,
            CleanupStatus::Skipped,
            "message_not_acknowledged_by_recipient",
        ),
        (
            &claude,
            message(&peer, Some(&receipt), Submission::Submitted, true),
            CleanupStatus::Unsupported,
            "client_has_no_notification_removal",
        ),
        (
            &peer,
            message(&peer, Some(&receipt), Submission::SubmissionUnknown, false),
            CleanupStatus::Pending,
            "notification_submission_has_no_completion_receipt",
        ),
        (
            &peer,
            message(&peer, Some(&receipt), Submission::SubmissionUnknown, true),
            CleanupStatus::Skipped,
            "no_confirmed_queue_receipt",
        ),
        (
            &peer,
            message(&peer, None, Submission::Submitted, true),
            CleanupStatus::Skipped,
            "no_confirmed_queue_receipt",
        ),
        (
            &peer,
            message(
                &peer,
                Some(&format!("codex_queued:{QUEUE_ID}")),
                Submission::Submitted,
                true,
            ),
            CleanupStatus::Skipped,
            "no_confirmed_queue_receipt",
        ),
        (
            &peer,
            message(
                &peer,
                Some("codex_cli_queued:invalid"),
                Submission::Submitted,
                true,
            ),
            CleanupStatus::Unavailable,
            "ValueError",
        ),
        (
            &peer,
            message(&peer, Some(&receipt), Submission::Submitted, true),
            CleanupStatus::Unavailable,
            "recipient_identity_changed",
        ),
    ];
    for (who, saved, status, detail) in cases {
        let cleanup = codex::dismiss(who, &saved);
        assert_eq!(
            (status, Some(detail)),
            (cleanup.status, cleanup.detail.as_deref())
        );
    }
    assert!(fixture.calls().is_empty(), "no client process was started");
    fixture.thread(&fixture.workspace(), 0, "cli");
    fs::remove_file(fixture.dir.join("bin/codex")).unwrap();
    let cleanup = codex::dismiss(
        &peer,
        &message(&peer, Some(&receipt), Submission::Submitted, true),
    );
    assert_eq!(
        (CleanupStatus::Unavailable, Some("FileNotFoundError")),
        (cleanup.status, cleanup.detail.as_deref())
    );
}

#[test]
fn stdio_shutdown_is_bounded_for_a_process_that_ignores_eof_and_sigterm() {
    let fixture = Fixture::new("stubborn");
    fixture.fake_codex();
    fixture.mode("stubborn");
    let mut rpc = Rpc::spawn_stdio().unwrap();
    assert_eq!(
        json!({"deleted": true}),
        rpc.call("thread/queue/delete", json!({})).unwrap()
    );
    let pid: i32 = fs::read_to_string(fixture.dir.join("bin/pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    drop(rpc);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1900) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );
    assert_eq!(
        -1,
        unsafe { libc::kill(pid, 0) },
        "the child was killed and reaped"
    );
}

const FAKE_CODEX: &str = r#"#!/usr/bin/python3 -B
import json, os, signal, sys, time
here = os.path.dirname(os.path.abspath(__file__))
def path(name): return os.path.join(here, name)
with open(path("calls.jsonl"), "a") as log: log.write(json.dumps(sys.argv[1:]) + "\n")
mode = open(path("mode")).read().strip() if os.path.exists(path("mode")) else ""
queue = "0f0e0d0c-0b0a-4908-8706-050403020100"
if sys.argv[1] == "queue":
    thread = sys.argv[3]
    out = {"ok": f"Queued message {queue} for thread {thread}.\n",
           "crlf": f"Queued message {queue} for thread {thread}.\r\n",
           "fail": f"Queued message {queue} for thread {thread}.\n",
           "garbage": "unexpected output\n",
           "other_thread": f"Queued message {queue} for thread 00000000-0000-4000-8000-000000000000.\n",
           "uppercase": f"Queued message {queue.upper()} for thread {thread}.\n",
           "hyphens": f"Queued message {'-' * 36} for thread {thread}.\n"}
    if mode == "latin1":
        sys.stdout.buffer.write(b"\xe9"); sys.exit(0)
    sys.stdout.write(out[mode]); sys.stdout.flush()
    sys.exit(1 if mode == "fail" else 0)
assert sys.argv[1:] == ["app-server", "--stdio"]
with open(path("pid"), "w") as f: f.write(str(os.getpid()))
if mode == "stubborn": signal.signal(signal.SIGTERM, signal.SIG_IGN)
if mode == "close_at_start":
    sys.stdin.readline(); sys.exit(0)
def send(frame):
    sys.stdout.write(json.dumps({"method": "irrelevant/notification"}) + "\n" + json.dumps(frame) + "\n"); sys.stdout.flush()
for raw in sys.stdin:
    frame = json.loads(raw)
    with open(path("frames.jsonl"), "a") as log: log.write(json.dumps(frame) + "\n")
    if frame["method"] == "initialized":
        continue
    if frame["method"] == "thread/queue/delete" and mode == "close_on_delete":
        sys.exit(0)
    if frame["method"] == "initialize":
        if mode == "huge":
            sys.stdout.write("x" * (5 * 1024 * 1024)); sys.stdout.flush(); time.sleep(5); sys.exit(0)
        send({"id": frame["id"], "result": {}}); continue
    result = {"removed": {"deleted": True}, "absent": {"deleted": False}, "invalid": {"deleted": "true"},
              "notdict": ["deleted"], "stubborn": {"deleted": True}}.get(mode)
    if mode == "rejected":
        send({"id": frame["id"], "error": {"code": -32000, "message": "secret /private/path"}})
    else:
        send({"id": frame["id"], "result": result})
with open(path("frames.jsonl"), "a") as log: log.write(json.dumps({"method": "exit"}) + "\n")
if mode == "stubborn":
    while True: time.sleep(1)
"#;
