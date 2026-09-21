//! Discovery sources against fixture rows, fake RPC calls, fixture lock tables and
//! fixture Codex state; never live clients, sockets of real sessions or /proc/locks.
use harness_talk::discovery::{
    Call, claude_sessions, codex_app_servers, codex_writers, finish, with_writers,
};
use harness_talk::error::Failure;
use harness_talk::model::Found;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
            "htalk-discovery-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn text(&self) -> String {
        self.0.to_str().unwrap().to_owned()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const ID: &str = "4b1f7f0e-2f4c-4a5e-9d1b-6f0c2d9e8a71";
const OTHER: &str = "0c4a1c0e-3a55-4c1f-a8f5-7d3b1f7e2a10";
const GONE: &str = "9d2e6b4a-1c3f-4e5d-8a7b-2f1e0d9c8b7a";

fn ids(found: &Found) -> Vec<&str> {
    found
        .sessions
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect()
}

// Claude agents

struct Claude {
    dir: TempDir,
    _listener: UnixListener,
    socket: String,
}
impl Claude {
    fn new() -> Self {
        let dir = TempDir::new();
        let socket = dir.path().join("123.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let socket = socket.to_str().unwrap().to_owned();
        let c = Self {
            dir,
            _listener: listener,
            socket,
        };
        c.metadata(123, json!({"sessionId": ID, "cwd": c.dir.text(), "pid": 123, "messagingSocketPath": c.socket}));
        c
    }
    fn row(&self, pid: i64) -> Value {
        json!({"sessionId": ID, "cwd": self.dir.text(), "pid": pid})
    }
    fn metadata(&self, pid: i64, value: Value) {
        fs::write(
            self.dir.path().join(format!("{pid}.json")),
            value.to_string(),
        )
        .unwrap();
    }
    fn discover(&self, rows: Vec<Value>) -> Found {
        let dir = self.dir.path().to_path_buf();
        claude_sessions(Ok(rows), &move |pid| {
            let text = fs::read_to_string(dir.join(format!("{pid}.json")))?;
            Ok(serde_json::from_str(&text)?)
        })
    }
}

#[test]
fn claude_verifies_each_address_and_malformed_rows_do_not_hide_later_ones() {
    let c = Claude::new();
    let found = c.discover(vec![
        json!({"broken": true}),
        json!([1]),
        json!("x"),
        c.row(123),
    ]);
    assert_eq!(vec![ID], ids(&found));
    assert_eq!(
        json!({"harness": "claude", "session_id": ID, "workspace": c.dir.text(), "runtime_status": "running",
                      "source": "claude_agents", "pid": 123}),
        found.sessions[0]
    );
    assert_eq!(
        json!({"harness": "claude", "source": "claude_agents", "status": "partial", "rejected": 3,
                      "detail": "Some live records could not be verified; run discovery again to refresh."}),
        found.sources[0]
    );
    let found = c.discover(vec![c.row(123)]);
    assert_eq!(
        json!({"harness": "claude", "source": "claude_agents", "status": "ok"}),
        found.sources[0]
    );
    // An uppercase listed ID is verified against its canonical form.
    let found = c.discover(vec![
        json!({"sessionId": ID.to_uppercase(), "cwd": c.dir.text(), "pid": 123}),
    ]);
    assert_eq!(vec![ID], ids(&found));
}

#[test]
fn claude_never_selects_one_of_duplicate_identities() {
    let c = Claude::new();
    let found = c.discover(vec![c.row(123), c.row(456)]);
    assert!(found.sessions.is_empty());
    assert_eq!(2, found.sources[0]["rejected"]);
}

#[test]
fn claude_rejects_changed_metadata_invalid_pids_and_non_sockets() {
    let c = Claude::new();
    c.metadata(
        123,
        json!({"sessionId": ID, "cwd": "/other", "pid": 123, "messagingSocketPath": c.socket}),
    );
    let found = c.discover(vec![c.row(123)]);
    assert!(found.sessions.is_empty());
    assert_eq!("partial", found.sources[0]["status"]);
    let plain = c.dir.path().join("plain");
    fs::write(&plain, "").unwrap();
    for saved in [
        json!({"sessionId": ID, "cwd": c.dir.text(), "messagingSocketPath": plain}),
        json!({"sessionId": ID, "cwd": c.dir.text()}),
        json!({"sessionId": ID, "cwd": c.dir.text(), "messagingSocketPath": 5}),
        json!([ID]),
    ] {
        c.metadata(123, saved.clone());
        assert!(c.discover(vec![c.row(123)]).sessions.is_empty(), "{saved}");
    }
    c.metadata(
        123,
        json!({"sessionId": ID, "cwd": c.dir.text(), "messagingSocketPath": c.socket}),
    );
    for row in [
        json!({"sessionId": ID, "cwd": c.dir.text(), "pid": 123.0}),
        json!({"sessionId": ID, "cwd": c.dir.text(), "pid": true}),
        json!({"sessionId": ID, "cwd": c.dir.text(), "pid": 0}),
        json!({"sessionId": ID, "cwd": c.dir.text()}),
        json!({"sessionId": ID, "cwd": "relative", "pid": 123}),
        json!({"sessionId": "nope", "cwd": c.dir.text(), "pid": 123}),
        json!({"sessionId": ID, "cwd": c.dir.text(), "pid": 999}),
    ] {
        assert_eq!(
            1,
            c.discover(vec![row.clone()]).sources[0]["rejected"],
            "{row}"
        );
    }
}

#[test]
fn claude_listing_failure_is_unavailable_with_the_class_name() {
    let never = |_| -> Result<Value, Failure> { panic!("no metadata without a listing") };
    for (failure, detail) in [
        (Failure::Class("FileNotFoundError"), "FileNotFoundError"),
        (Failure::Class("CalledProcessError"), "CalledProcessError"),
        (
            Failure::coded("invalid_claude_agents_response"),
            "ValueError",
        ),
    ] {
        let found = claude_sessions(Err(failure), &never);
        assert!(found.sessions.is_empty());
        assert_eq!(
            json!([{"harness": "claude", "source": "claude_agents", "status": "unavailable", "detail": detail}]),
            json!(found.sources)
        );
    }
    assert_eq!(
        "ok",
        claude_sessions(Ok(vec![]), &never).sources[0]["status"]
    );
}

// Codex app-server

type Log = Rc<RefCell<Vec<(String, Value)>>>;

fn server(
    log: Log,
    answer: impl Fn(&str, &Value) -> Result<Value, Failure> + 'static,
) -> impl Fn(&Path) -> Result<Call<'static>, Failure> {
    let answer = Rc::new(answer);
    move |_path: &Path| {
        let (log, answer) = (log.clone(), answer.clone());
        Ok(Box::new(move |method: &str, params: Value| {
            log.borrow_mut().push((method.to_owned(), params.clone()));
            answer(method, &params)
        }) as Call<'static>)
    }
}

fn thread(id: &str, cwd: &str, status: &str) -> Value {
    json!({"thread": {"id": id, "cwd": cwd, "status": {"type": status}}})
}

#[test]
fn codex_pages_loaded_ids_without_turns_and_skips_unloaded_threads() {
    let dir = TempDir::new();
    let cwd = dir.text();
    let log = Log::default();
    let connect = server(log.clone(), move |method, params| {
        Ok(match method {
            "thread/loaded/list" if params["cursor"].is_null() => {
                json!({"data": [ID, GONE], "nextCursor": "next"})
            }
            "thread/loaded/list" => json!({"data": [OTHER], "nextCursor": null}),
            "thread/read" => {
                let id = params["threadId"].as_str().unwrap();
                thread(id, &cwd, if id == GONE { "notLoaded" } else { "idle" })
            }
            other => panic!("{other}"),
        })
    });
    let socket = dir.path().join("codex.sock").to_str().unwrap().to_owned();
    let found = codex_app_servers(std::slice::from_ref(&socket), &connect);
    assert_eq!(vec![ID, OTHER], ids(&found));
    assert_eq!(
        json!({"harness": "codex", "session_id": ID, "workspace": dir.text(), "runtime_status": "idle",
                      "source": "codex_app_server", "socket": socket}),
        found.sessions[0]
    );
    assert_eq!(
        json!([{"harness": "codex", "source": "codex_app_server", "socket": socket, "status": "ok"}]),
        json!(found.sources)
    );
    let log = log.borrow();
    assert_eq!(json!({"cursor": null, "limit": 100}), log[0].1);
    assert_eq!(
        2,
        log.iter()
            .filter(|(m, _)| m == "thread/loaded/list")
            .count()
    );
    assert!(
        log.iter()
            .filter(|(m, _)| m == "thread/read")
            .all(|(_, p)| p["includeTurns"] == false)
    );
}

#[test]
fn codex_rejects_bad_records_without_hiding_later_ones() {
    let dir = TempDir::new();
    let cwd = dir.text();
    let worker = "1a2b3c4d-5e6f-4a1b-8c2d-3e4f5a6b7c8d";
    let rejected_rpc = "2b3c4d5e-6f7a-4b2c-9d3e-4f5a6b7c8d9e";
    let unknown = "3c4d5e6f-7a8b-4c3d-8e4f-5a6b7c8d9e0f";
    let log = Log::default();
    let connect = server(log.clone(), move |method, params| {
        if method == "thread/loaded/list" {
            return Ok(
                json!({"data": [5, "bad", worker, rejected_rpc, unknown, GONE, ID, ID.to_uppercase()]}),
            );
        }
        let id = params["threadId"].as_str().unwrap();
        Ok(match id {
            _ if id == worker => {
                json!({"thread": {"id": id, "cwd": cwd, "status": {"type": "idle"}, "canAcceptDirectInput": false}})
            }
            _ if id == rejected_rpc => return Err(Failure::coded("codex_rpc_rejected:-32600")),
            _ if id == unknown => thread(id, &cwd, "sleeping"),
            _ if id == GONE => thread(OTHER, &cwd, "idle"),
            _ => thread(id, &cwd, "active"),
        })
    });
    let found = codex_app_servers(&[dir.text()], &connect);
    assert_eq!(vec![ID], ids(&found));
    assert_eq!("active", found.sessions[0]["runtime_status"]);
    // 5, "bad", the RPC rejection, the unknown status and the changed identity.
    assert_eq!(
        json!({"harness": "codex", "source": "codex_app_server", "socket": dir.text(), "status": "partial",
                      "rejected": 5, "detail": "Some loaded records could not be verified."}),
        found.sources[0]
    );
    // The duplicate (uppercase) ID is read once.
    assert_eq!(
        1,
        log.borrow()
            .iter()
            .filter(|(_, p)| p["threadId"] == ID)
            .count()
    );
}

#[test]
fn codex_partial_results_survive_a_later_transport_failure() {
    let dir = TempDir::new();
    let cwd = dir.text();
    let connect = server(Log::default(), move |method, params| match method {
        "thread/loaded/list" if params["cursor"].is_null() => {
            Ok(json!({"data": [ID], "nextCursor": "next"}))
        }
        "thread/loaded/list" => Err(Failure::Class("OSError")),
        _ => Ok(thread(ID, &cwd, "active")),
    });
    let found = codex_app_servers(&[dir.text()], &connect);
    assert_eq!(vec![ID], ids(&found));
    assert_eq!(
        json!({"harness": "codex", "source": "codex_app_server", "socket": dir.text(), "status": "partial", "detail": "OSError"}),
        found.sources[0]
    );
    // An RPC timeout during a read ends the source rather than counting a record.
    let connect = server(Log::default(), |method, _| match method {
        "thread/loaded/list" => Ok(json!({"data": [ID]})),
        _ => Err(Failure::coded("codex_rpc_timeout")),
    });
    let found = codex_app_servers(&[dir.text()], &connect);
    assert_eq!("unavailable", found.sources[0]["status"]);
    assert_eq!("codex_rpc_timeout", found.sources[0]["detail"]);
}

#[test]
fn codex_unavailable_socket_has_next_action_and_duplicate_paths_are_one_source() {
    let dir = TempDir::new();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let connect =
        |_: &Path| -> Result<Call<'static>, Failure> { Err(Failure::Class("FileNotFoundError")) };
    let path = dir.path().join("codex.sock");
    let found = codex_app_servers(
        &[
            path.to_str().unwrap().into(),
            format!("{}/sub/../codex.sock", dir.text()),
        ],
        &connect,
    );
    assert!(found.sessions.is_empty());
    assert_eq!(1, found.sources.len());
    assert_eq!(
        json!({"harness": "codex", "source": "codex_app_server", "socket": path.to_str().unwrap(), "status": "unavailable",
        "detail": "FileNotFoundError", "next_action": "Check the running Codex app-server socket and permissions, or pass --codex-socket. \
        Embedded clients without a socket are outside this source's coverage."}),
        found.sources[0]
    );
    let found = codex_app_servers(&[], &connect);
    assert!(found.sources.is_empty());
}

#[test]
fn codex_bounds_cursors_and_inspected_ids() {
    let dir = TempDir::new();
    let cwd = dir.text();
    let repeat = server(Log::default(), move |method, _| {
        Ok(match method {
            "thread/loaded/list" => json!({"data": [ID], "nextCursor": "same"}),
            _ => thread(ID, &cwd, "idle"),
        })
    });
    let found = codex_app_servers(&[dir.text()], &repeat);
    assert_eq!(vec![ID], ids(&found));
    assert_eq!(
        ("partial", "invalid_discovery_cursor"),
        (
            found.sources[0]["status"].as_str().unwrap(),
            found.sources[0]["detail"].as_str().unwrap()
        )
    );
    let page = server(Log::default(), |_, _| {
        Ok(json!({"data": [], "nextCursor": 7}))
    });
    assert_eq!(
        "invalid_discovery_cursor",
        codex_app_servers(&[dir.text()], &page).sources[0]["detail"]
    );
    let page = server(Log::default(), |_, _| Ok(json!({"data": {}})));
    assert_eq!(
        "invalid_loaded_threads_response",
        codex_app_servers(&[dir.text()], &page).sources[0]["detail"]
    );
    // At most 20 distinct cursors.
    let counter = Rc::new(RefCell::new(0));
    let pages = counter.clone();
    let cursors = server(Log::default(), move |_, _| {
        *pages.borrow_mut() += 1;
        Ok(json!({"data": [], "nextCursor": format!("c{}", pages.borrow())}))
    });
    assert_eq!(
        "invalid_discovery_cursor",
        codex_app_servers(&[dir.text()], &cursors).sources[0]["detail"]
    );
    assert_eq!(21, *counter.borrow());
    // At most 200 inspected IDs.
    let cwd = dir.text();
    let many: Vec<String> = (0..201)
        .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
        .collect();
    let limit = server(Log::default(), move |method, params| {
        Ok(match method {
            "thread/loaded/list" => json!({"data": many}),
            _ => thread(params["threadId"].as_str().unwrap(), &cwd, "idle"),
        })
    });
    let found = codex_app_servers(&[dir.text()], &limit);
    assert_eq!(200, found.sessions.len());
    assert_eq!(
        ("partial", "codex_discovery_limit"),
        (
            found.sources[0]["status"].as_str().unwrap(),
            found.sources[0]["detail"].as_str().unwrap()
        )
    );
}

// Codex writer locks

struct Writers {
    dir: TempDir,
    locks: PathBuf,
    lines: RefCell<Vec<String>>,
}
impl Writers {
    fn new() -> Self {
        let dir = TempDir::new();
        let locks = dir.path().join("thread-writer-locks");
        fs::create_dir(&locks).unwrap();
        Self {
            dir,
            locks,
            lines: RefCell::default(),
        }
    }
    fn file(&self, ident: &str) -> PathBuf {
        let path = self.locks.join(format!("{ident}.lock"));
        fs::write(&path, "").unwrap();
        path
    }
    fn hold(&self, path: &Path, pid: i64, kind: &str) {
        let info = fs::metadata(path).unwrap();
        let line = format!(
            "{}: {kind} {pid} {:x}:{:x}:{} 0 EOF",
            self.lines.borrow().len() + 1,
            libc::major(info.dev()),
            libc::minor(info.dev()),
            info.ino()
        );
        self.lines.borrow_mut().push(line);
    }
    fn state(&self, rows: &[(&str, Value, Value, &str)]) -> PathBuf {
        let path = self.dir.path().join("state_5.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute(
            "CREATE TABLE threads (id TEXT, cwd TEXT, archived INTEGER, source TEXT)",
            [],
        )
        .unwrap();
        for (id, cwd, archived, source) in rows {
            let sql = |v: &Value| match v {
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                Value::Null => rusqlite::types::Value::Null,
                Value::Number(n) if n.is_i64() => {
                    rusqlite::types::Value::Integer(n.as_i64().unwrap())
                }
                _ => rusqlite::types::Value::Real(v.as_f64().unwrap()),
            };
            db.execute(
                "INSERT INTO threads VALUES (?, ?, ?, ?)",
                rusqlite::params![id, sql(cwd), sql(archived), source],
            )
            .unwrap();
        }
        path
    }
    fn scan(&self, state: Option<PathBuf>) -> Found {
        let table = self.dir.path().join("locks");
        fs::write(&table, self.lines.borrow().join("\n")).unwrap();
        codex_writers(&self.locks, &table, move || {
            Ok(state.expect("state is read only for held writers"))
        })
    }
}

#[test]
fn native_codex_requires_a_held_write_lock_and_a_saved_unarchived_cli_address() {
    let w = Writers::new();
    let cwd = json!(w.dir.text());
    let (stale, worker, archived, reader, blocked, idle) = (
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
    );
    w.file(stale);
    let held = w.file(ID);
    w.hold(&held, 4321, "FLOCK  ADVISORY  WRITE");
    w.hold(&w.file(worker), 10, "FLOCK ADVISORY WRITE");
    w.hold(&w.file(archived), 11, "FLOCK ADVISORY WRITE");
    w.hold(&w.file(reader), 12, "FLOCK ADVISORY READ");
    w.hold(&w.file(blocked), 13, "-> FLOCK ADVISORY WRITE");
    w.hold(&w.file(idle), 0, "FLOCK ADVISORY WRITE");
    w.hold(&held, 1, "POSIX ADVISORY WRITE");
    let state = w.state(&[
        (ID, cwd.clone(), json!(0), "cli"),
        (stale, cwd.clone(), json!(0), "cli"),
        (worker, cwd.clone(), json!(0), "subAgent"),
        (archived, cwd.clone(), json!(1), "cli"),
        (reader, cwd.clone(), json!(0), "cli"),
        (blocked, cwd.clone(), json!(0), "cli"),
        (idle, cwd.clone(), json!(0), "cli"),
    ]);
    let found = w.scan(Some(state.clone()));
    assert_eq!(vec![ID], ids(&found));
    assert_eq!(
        json!({"harness": "codex", "session_id": ID, "workspace": w.dir.text(), "runtime_status": "writer_active",
        "runtime_reason": "kernel_writer_lock", "source": "codex_writer_locks", "pid": 4321, "socket": null}),
        found.sessions[0]
    );
    assert_eq!(
        json!({"harness": "codex", "source": "codex_writer_locks", "status": "ok", "path": w.locks.to_str().unwrap(),
        "scope": "CLI writers visible in this Linux process namespace."}),
        found.sources[0]
    );
    // The lock file is only inspected, never removed.
    assert!(held.exists());
    // Without a held lock the state is not even opened.
    w.lines.borrow_mut().clear();
    w.hold(&held, 4321, "FLOCK ADVISORY READ");
    let found = w.scan(None);
    assert!(found.sessions.is_empty());
    assert_eq!("ok", found.sources[0]["status"]);
}

#[test]
fn bad_saved_writer_does_not_hide_a_later_valid_address() {
    let w = Writers::new();
    let (bad, missing) = (
        "77777777-7777-4777-8777-777777777777",
        "88888888-8888-4888-8888-888888888888",
    );
    for ident in [bad, missing, ID] {
        w.hold(&w.file(ident), 123, "FLOCK ADVISORY WRITE");
    }
    let state = w.state(&[
        (bad, Value::Null, json!(0), "cli"),
        (ID, json!(w.dir.text()), json!(0.0), "cli"),
    ]);
    let found = w.scan(Some(state));
    assert_eq!(vec![ID], ids(&found));
    let source = &found.sources[0];
    assert_eq!(
        ("partial", 1),
        (
            source["status"].as_str().unwrap(),
            source["rejected"].as_i64().unwrap()
        )
    );
    assert_eq!(
        "Some held writers have no saved address yet.",
        source["detail"]
    );
}

#[test]
fn writer_lock_files_must_be_owned_regular_uuid_files() {
    let w = Writers::new();
    let target = w.dir.path().join("target");
    fs::write(&target, "").unwrap();
    let hidden = w.locks.join(format!(".{ID}.lock"));
    fs::write(&hidden, "").unwrap();
    w.hold(&hidden, 5, "FLOCK ADVISORY WRITE");
    let link = w.locks.join(format!("{OTHER}.lock"));
    symlink(&target, &link).unwrap();
    w.hold(&target, 6, "FLOCK ADVISORY WRITE");
    let odd = w.locks.join("not-a-uuid.lock");
    fs::write(&odd, "").unwrap();
    w.hold(&odd, 7, "FLOCK ADVISORY WRITE");
    let found = w.scan(None);
    assert!(found.sessions.is_empty());
    assert_eq!("ok", found.sources[0]["status"]);
}

#[test]
fn writer_source_failures_are_class_names() {
    let w = Writers::new();
    w.hold(&w.file(ID), 9, "FLOCK ADVISORY WRITE");
    // A missing state database is unavailable, not empty success.
    let found = w.scan(Some(w.dir.path().join("absent/state_5.sqlite")));
    assert_eq!(
        ("unavailable", "OperationalError"),
        (
            found.sources[0]["status"].as_str().unwrap(),
            found.sources[0]["detail"].as_str().unwrap()
        )
    );
    // A malformed lock record ends the scan.
    w.lines
        .borrow_mut()
        .push("9: FLOCK ADVISORY WRITE 9 zz 0 EOF".into());
    assert_eq!("ValueError", w.scan(None).sources[0]["detail"]);
    let absent = codex_writers(
        &w.dir.path().join("absent"),
        Path::new("/nonexistent-locks"),
        || panic!("not reached"),
    );
    assert_eq!(
        ("unavailable", "FileNotFoundError"),
        (
            absent.sources[0]["status"].as_str().unwrap(),
            absent.sources[0]["detail"].as_str().unwrap()
        )
    );
    let table = codex_writers(&w.locks, &w.dir.path().join("no-table"), || {
        panic!("not reached")
    });
    assert_eq!("FileNotFoundError", table.sources[0]["detail"]);
}

// Grouping and filtering

fn candidate(harness: &str, id: &str, workspace: &str) -> Value {
    json!({"harness": harness, "session_id": id, "workspace": workspace})
}

#[test]
fn writer_failure_does_not_hide_app_server_results_and_duplicates_are_merged() {
    let app = Found {
        sessions: vec![candidate("codex", ID, "/w")],
        sources: vec![json!({"harness": "codex", "status": "ok"})],
    };
    let writers = Found {
        sessions: vec![],
        sources: vec![json!({"harness": "codex", "status": "unavailable"})],
    };
    let result = finish(with_writers(app, Some(writers)), None);
    assert_eq!(json!([candidate("codex", ID, "/w")]), result["sessions"]);
    assert_eq!(
        vec!["ok", "unavailable"],
        result["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["status"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    let app = Found {
        sessions: vec![candidate("codex", ID, "/w")],
        sources: vec![],
    };
    let writers = Found {
        sessions: vec![
            candidate("codex", ID, "/w"),
            candidate("codex", ID, "/v"),
            candidate("codex", OTHER, "/w"),
        ],
        sources: vec![],
    };
    assert_eq!(3, with_writers(app, Some(writers)).sessions.len());
    let app = Found {
        sessions: vec![candidate("codex", ID, "/w")],
        sources: vec![],
    };
    assert_eq!(1, with_writers(app, None).sessions.len());
}

#[test]
fn workspace_filter_accepts_native_symlinks_and_results_are_sorted() {
    let dir = TempDir::new();
    let actual = dir.path().join("project");
    fs::create_dir(&actual).unwrap();
    let alias = dir.path().join("alias");
    symlink(&actual, &alias).unwrap();
    let alias = alias.to_str().unwrap();
    let rows = || Found {
        sessions: vec![
            candidate("opencode", "ses1", alias),
            candidate("codex", OTHER, "/other"),
            candidate("codex", ID, alias),
            candidate("claude", ID, "relative"),
            json!({"harness": "claude", "session_id": OTHER}),
        ],
        sources: vec![],
    };
    let resolved = actual.to_str().unwrap();
    let result = finish(rows(), Some(resolved));
    assert_eq!(
        json!([
            candidate("codex", ID, alias),
            candidate("opencode", "ses1", alias)
        ]),
        result["sessions"]
    );
    let result = finish(rows(), None);
    let order: Vec<_> = result["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["harness"].as_str().unwrap(),
                s["workspace"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        vec![
            ("claude", ""),
            ("claude", "relative"),
            ("codex", "/other"),
            ("codex", alias),
            ("opencode", alias)
        ],
        order
    );
    assert!(result["scope"].as_str().unwrap().contains("do not prove"));
    assert!(result["next_action"].as_str().unwrap().contains("peer add"));
    assert_eq!(
        vec!["sessions", "sources", "next_action", "scope"],
        result.as_object().unwrap().keys().collect::<Vec<_>>()
    );
}
