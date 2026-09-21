//! Read-only OpenCode discovery: server sources, saved metadata and per-record diagnostics.
#[path = "opencode_fixture.rs"]
mod fixture;

use fixture::{Fake, Reply, closed_port, session};
use harness_talk::opencode::{self, discover_with};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::Path;

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}
fn ids(found: &[Value]) -> Vec<&str> {
    found
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect()
}
fn workspace(dir: &fixture::TempDir) -> String {
    dir.path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}
/// id, parent_id, directory, time_updated, time_archived.
type SavedRow<'a> = (Value, Option<&'a str>, Value, Value, Option<i64>);
fn saved_db(path: &Path, rows: &[SavedRow<'_>]) {
    let db = Connection::open(path).unwrap();
    db.execute("CREATE TABLE session (id, parent_id TEXT, directory, time_updated, time_archived INTEGER, title TEXT)", []).unwrap();
    for (id, parent, directory, updated, archived) in rows {
        let sql = |v: &Value| -> rusqlite::types::Value {
            match v {
                Value::Null => rusqlite::types::Value::Null,
                Value::String(s) => s.clone().into(),
                Value::Number(n) if n.is_i64() => n.as_i64().unwrap().into(),
                Value::Number(n) => n.as_f64().unwrap().into(),
                other => panic!("{other}"),
            }
        };
        db.execute(
            "INSERT INTO session VALUES (?, ?, ?, ?, ?, 'private title')",
            params![sql(id), parent, sql(directory), sql(updated), archived],
        )
        .unwrap();
    }
}

#[test]
fn malformed_server_records_preserve_valid_sessions_before_and_after() {
    let dir = fixture::tempdir();
    let ws = workspace(&dir);
    let fake = Fake::start(vec![
        session("ses_first", ws.as_str()),
        session("bad_id", ws.as_str()),
        session("ses_bad_directory", Value::Null),
        json!({"id": "ses_bad_time", "directory": ws, "time": []}),
        session("ses_bad_status", ws.as_str()),
        json!("not an object"),
        session("ses_last", ws.as_str()),
    ]);
    fake.with(|s| {
        s.status =
            json!({"ses_bad_status": {"type": "unrecognized"}, "ses_last": {"type": "retry"}})
    });
    let result = discover_with(
        Some(std::slice::from_ref(&fake.url)),
        None,
        Some(&dir.path().join("absent.db")),
    );
    assert_eq!(ids(&result.sessions), ["ses_first", "ses_last"]);
    let source = &result.sources[0];
    assert_eq!(
        keys(source),
        [
            "harness", "source", "url", "status", "version", "error", "detail", "rejected"
        ]
    );
    assert_eq!(
        (
            source["status"].as_str(),
            source["rejected"].as_i64(),
            source["detail"].as_str(),
            source["version"].as_str()
        ),
        (
            Some("partial"),
            Some(5),
            Some("opencode_invalid_session_records"),
            Some("1.18.30")
        )
    );
    let last = &result.sessions[1];
    assert_eq!(
        keys(last),
        [
            "harness",
            "session_id",
            "workspace",
            "runtime_status",
            "runtime_reason",
            "source",
            "url",
            "updated_at"
        ]
    );
    assert_eq!(
        last,
        &json!({"harness": "opencode", "session_id": "ses_last", "workspace": ws, "runtime_status": "retry",
        "runtime_reason": "server_status", "source": "opencode_server", "url": fake.url, "updated_at": 1788990000})
    );
    // Without a workspace, the server's own project answers: no directory parameter.
    assert!(
        fake.requests()
            .iter()
            .all(|r| r.query.is_empty() && r.method == "GET")
    );
    assert!(
        !serde_json::to_string(&result.sessions)
            .unwrap()
            .contains("synthetic\"")
    ); // No titles or slugs.
}

#[test]
fn discovery_is_read_only_and_separates_liveness() {
    let dir = fixture::tempdir();
    let ws = workspace(&dir);
    let mut child = session("ses_child", ws.as_str());
    child["parentID"] = json!("ses_synthetic");
    let mut gone = session("ses_gone", ws.as_str());
    gone["time"]["archived"] = json!(3);
    let mut odd = session("ses_odd_time", format!("{ws}/other"));
    odd["time"]["updated"] = json!(-1500);
    let fake = Fake::start(vec![
        session("ses_synthetic", ws.as_str()),
        child,
        gone,
        session("ses_busy", format!("{ws}/other")),
        odd,
    ]);
    fake.with(|s| s.status = json!({"ses_busy": {"type": "busy"}}));
    let saved = dir.path().join("opencode.db");
    saved_db(
        &saved,
        &[
            (
                json!("ses_synthetic"),
                None,
                json!(ws),
                json!(1788990000000i64),
                None,
            ),
            (
                json!("ses_saved"),
                None,
                json!("/synthetic/saved"),
                json!(1788980000000i64),
                None,
            ),
            (
                json!("ses_archived"),
                None,
                json!("/synthetic/x"),
                json!(5),
                Some(6),
            ),
            (
                json!("ses_subagent"),
                Some("ses_saved"),
                json!("/synthetic/saved"),
                json!(7),
                None,
            ),
        ],
    );
    let before = std::fs::read(&saved).unwrap();
    let urls = [
        "http://user:sensitive-secret@127.0.0.1:4096".to_owned(),
        "http://127.attacker.example:4096".to_owned(),
        fake.url.clone(),
        format!("http://127.0.0.1:{}", closed_port()),
    ];
    let result = discover_with(Some(&urls), Some(&ws), Some(&saved));
    let text =
        serde_json::to_string(&json!({"sessions": result.sessions, "sources": result.sources}))
            .unwrap();
    assert!(
        !text.contains("sensitive-secret")
            && !text.contains("attacker")
            && !text.contains("private title")
    );
    let summary: Vec<(&str, &str, &str, Value)> = result
        .sources
        .iter()
        .map(|s| {
            (
                s["source"].as_str().unwrap(),
                s["status"].as_str().unwrap(),
                s["error"].as_str().unwrap_or("-"),
                s.get("url").cloned().unwrap_or(json!("-")),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "opencode_server",
                "unavailable",
                "invalid_opencode_url",
                Value::Null
            ),
            (
                "opencode_server",
                "unavailable",
                "opencode_url_must_be_loopback",
                Value::Null
            ),
            ("opencode_server", "ok", "-", json!(fake.url)),
            (
                "opencode_server",
                "unavailable",
                "opencode_unreachable",
                json!(urls[3])
            ),
            ("opencode_saved", "ok", "-", json!("-"))
        ]
    );
    assert!(result.sources.iter().all(|s| s["harness"] == "opencode"));
    assert_eq!(result.sources[4]["path"].as_str(), saved.to_str());
    assert_eq!(
        ids(&result.sessions),
        ["ses_synthetic", "ses_busy", "ses_odd_time", "ses_saved"]
    );
    let busy = &result.sessions[1];
    assert_eq!(
        (
            busy["runtime_status"].as_str(),
            result.sessions[0]["runtime_status"].as_str()
        ),
        (Some("busy"), Some("idle"))
    );
    assert_eq!(result.sessions[0]["source"], "opencode_server"); // A server record wins over its saved duplicate.
    assert_eq!(result.sessions[2]["updated_at"], -2); // Floor division, as Python's //.
    assert_eq!(
        result.sessions[3],
        json!({"harness": "opencode", "session_id": "ses_saved", "workspace": "/synthetic/saved",
        "runtime_status": "unknown", "runtime_reason": "saved_metadata_only", "source": "opencode_saved", "url": null, "updated_at": 1788980000})
    );
    let scoped: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.path == "/session" || r.path == "/session/status")
        .collect();
    assert_eq!(scoped.len(), 2);
    assert!(scoped.iter().all(|r| r.query.get("directory") == Some(&ws)));
    assert!(fake.posts().is_empty());
    assert_eq!(before, std::fs::read(&saved).unwrap());
}

#[test]
fn server_source_failures_are_confined_and_coded() {
    let dir = fixture::tempdir();
    let fake = Fake::start(vec![]);
    let absent = dir.path().join("absent.db");
    let error = |hook: Option<fn() -> Reply>| {
        match hook {
            Some(reply) => fake.hook(move |r| (r.path == "/global/health").then(reply)),
            None => fake.with(|s| s.hook = None),
        }
        let result = discover_with(Some(std::slice::from_ref(&fake.url)), None, Some(&absent));
        assert!(result.sessions.is_empty());
        let s = &result.sources[0];
        assert_eq!(
            keys(s),
            [
                "harness", "source", "url", "status", "version", "error", "detail"
            ]
        );
        (
            s["status"].as_str().unwrap().to_owned(),
            s["error"].as_str().unwrap_or("-").to_owned(),
        )
    };
    fake.with(|s| s.password = Some("synthetic-secret".into()));
    assert_eq!(
        error(None),
        ("unavailable".into(), "opencode_unauthorized".into())
    );
    fake.with(|s| s.password = None);
    assert_eq!(
        error(Some(|| Reply::Raw(
            format!(
                "HTTP/1.1 200 OK\r\n\r\n{}",
                " ".repeat(opencode::MAX_RESPONSE + 1)
            )
            .into_bytes()
        ))),
        ("unavailable".into(), "opencode_response_too_large".into())
    );
    assert_eq!(
        error(Some(|| Reply::Close)),
        ("unavailable".into(), "opencode_RemoteDisconnected".into())
    );
    assert_eq!(
        error(Some(|| Reply::Json(200, json!({"healthy": true})))),
        ("unavailable".into(), "opencode_invalid_response".into())
    );
    fake.hook(|r| (r.path == "/session").then(|| Reply::Json(200, json!({"not": "a list"}))));
    let result = discover_with(Some(std::slice::from_ref(&fake.url)), None, Some(&absent));
    assert_eq!(
        (
            result.sources[0]["status"].as_str(),
            result.sources[0]["error"].as_str(),
            result.sources[0]["version"].as_str()
        ),
        (
            Some("unavailable"),
            Some("opencode_invalid_response"),
            Some("1.18.30")
        )
    );
    fake.hook(|r| (r.path == "/session/status").then(|| Reply::Json(404, json!({}))));
    let result = discover_with(Some(std::slice::from_ref(&fake.url)), None, Some(&absent));
    assert_eq!(
        result.sources[0]["error"],
        "recipient_not_in_opencode_server"
    );
    // An empty URL list still reads saved metadata; the missing file is not created.
    let result = discover_with(Some(&[]), None, Some(&absent));
    assert_eq!(
        result.sources,
        [
            json!({"harness": "opencode", "source": "opencode_saved", "path": absent.to_str(),
        "status": "unavailable", "error": "opencode_saved_metadata_missing", "detail": null})
        ]
    );
    assert!(!absent.exists());
}

#[test]
fn malformed_saved_rows_preserve_valid_addresses_and_limit_diagnostics() {
    let dir = fixture::tempdir();
    let ws = workspace(&dir);
    let saved = dir.path().join("opencode.db");
    saved_db(
        &saved,
        &[
            (json!("ses_first"), None, json!(ws), json!(5000), None),
            (json!("bad_id"), None, json!(ws), json!(4000), None),
            (json!("ses_empty"), None, json!(""), json!(3000), None),
            (json!(17), None, json!(ws), json!(2500), None),
            (json!("ses_last"), None, json!(ws), json!(2000.5), None),
            (json!("ses_older"), None, json!(ws), Value::Null, None),
        ],
    );
    let before = std::fs::read(&saved).unwrap();
    let result = discover_with(Some(&[]), None, Some(&saved));
    assert_eq!(
        ids(&result.sessions),
        ["ses_first", "ses_last", "ses_older"]
    );
    let source = &result.sources[0];
    assert_eq!(
        keys(source),
        [
            "harness", "source", "path", "status", "error", "detail", "rejected"
        ]
    );
    assert_eq!(
        (
            source["status"].as_str(),
            source["rejected"].as_i64(),
            source["detail"].as_str()
        ),
        (
            Some("partial"),
            Some(3),
            Some("opencode_invalid_saved_metadata")
        )
    );
    assert_eq!(
        (
            result.sessions[0]["updated_at"].as_i64(),
            &result.sessions[1]["updated_at"]
        ),
        (Some(5), &Value::Null)
    );
    assert_eq!(before, std::fs::read(&saved).unwrap());

    // 51 unarchived roots: the newest 50 are read, and two of them are malformed.
    let limited = dir.path().join("limited.db");
    let rows: Vec<_> = (0..51)
        .map(|i| {
            (
                json!(if i == 3 {
                    "bad".to_owned()
                } else {
                    format!("ses_{i:02}")
                }),
                None,
                json!(if i == 7 { String::new() } else { ws.clone() }),
                json!(10_000 - i),
                None,
            )
        })
        .collect();
    saved_db(&limited, &rows);
    let result = discover_with(Some(&[]), None, Some(&limited));
    assert_eq!(result.sessions.len(), 48);
    assert!(!ids(&result.sessions).contains(&"ses_50"));
    let source = &result.sources[0];
    assert_eq!(
        (
            source["status"].as_str(),
            source["rejected"].as_i64(),
            source["detail"].as_str()
        ),
        (
            Some("partial"),
            Some(2),
            Some("opencode_saved_session_limit_reached")
        )
    );
    let exact = dir.path().join("exact.db");
    saved_db(
        &exact,
        &rows[..50]
            .iter()
            .filter(|r| r.0 != "bad" && r.2 != "")
            .cloned()
            .collect::<Vec<_>>(),
    );
    let source = &discover_with(Some(&[]), None, Some(&exact)).sources[0];
    assert_eq!(
        (source["status"].as_str(), source["detail"].as_str()),
        (Some("ok"), None)
    );
}

#[test]
fn unreadable_saved_metadata_is_unavailable_by_class() {
    let dir = fixture::tempdir();
    let garbage = dir.path().join("garbage.db");
    std::fs::write(&garbage, vec![b'x'; 4096]).unwrap();
    let no_table = dir.path().join("empty.db");
    Connection::open(&no_table)
        .unwrap()
        .execute("CREATE TABLE other (x)", [])
        .unwrap();
    let bad_text = dir.path().join("text.db");
    saved_db(
        &bad_text,
        &[(json!("ses_ok"), None, json!("/synthetic"), json!(1), None)],
    );
    Connection::open(&bad_text)
        .unwrap()
        .execute(
            "INSERT INTO session VALUES (CAST(x'736573ff' AS TEXT), NULL, '/x', 0, NULL, '')",
            [],
        )
        .unwrap();
    for (path, error) in [
        (&garbage, "opencode_saved_DatabaseError"),
        (&no_table, "opencode_saved_OperationalError"),
        (&bad_text, "opencode_saved_OperationalError"),
        (&dir.path().to_path_buf(), "opencode_saved_metadata_missing"),
    ] {
        let result = discover_with(Some(&[]), None, Some(path));
        assert!(result.sessions.is_empty());
        assert_eq!(
            (
                result.sources[0]["status"].as_str(),
                result.sources[0]["error"].as_str(),
                &result.sources[0]["detail"]
            ),
            (Some("unavailable"), Some(error), &Value::Null),
            "{path:?}"
        );
    }
    // The source path is reported as pathlib prints it.
    let spaced = format!("{}//./opencode.db", dir.path().display());
    let result = discover_with(Some(&[]), None, Some(Path::new(&spaced)));
    assert_eq!(
        result.sources[0]["path"].as_str(),
        dir.path().join("opencode.db").to_str()
    );
}
