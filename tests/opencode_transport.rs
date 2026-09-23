//! OpenCode probe and one-attempt delivery against a synthetic loopback server.
#[path = "opencode_fixture.rs"]
mod fixture;

use fixture::{Fake, Reply, closed_port, session};
use harness_talk::model::NativePeer as Peer;
use harness_talk::{error::Failure, model::*, opencode};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

struct Case {
    _dir: fixture::TempDir,
    workspace: String,
    fake: Fake,
    peer: Peer,
}

fn case() -> Case {
    let dir = fixture::tempdir();
    let workspace = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let fake = Fake::start(vec![session("ses_synthetic", workspace.as_str())]);
    let peer = Peer {
        name: "muse".into(),
        harness: Harness::Opencode,
        session_id: "ses_synthetic".into(),
        workspace: workspace.clone(),
        socket: None,
        url: Some(fake.url.clone()),
        retired_at: None,
    };
    Case {
        _dir: dir,
        workspace,
        fake,
        peer,
    }
}
fn no_skip() -> Result<Option<SkipReason>, Failure> {
    Ok(None)
}
fn outcome(o: Outcome) -> (&'static str, String) {
    (o.submission.as_str(), o.detail)
}
fn err(r: Result<Value, Failure>) -> String {
    r.expect_err("expected a failure").to_string()
}
fn raw(text: &str) -> Reply {
    Reply::Raw(text.as_bytes().to_vec())
}
fn health_hook(c: &Case, reply: impl Fn() -> Reply + Send + 'static) {
    c.fake
        .hook(move |r| (r.path == "/global/health").then(&reply));
}

#[test]
fn probe_checks_exact_session_workspace_and_status() {
    let c = case();
    let result = opencode::probe(&c.peer).unwrap();
    let keys: Vec<&str> = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "harness",
            "session_id",
            "workspace",
            "url",
            "server_version",
            "runtime_status",
            "transport",
            "authenticated"
        ]
    );
    assert_eq!(
        (
            result["session_id"].as_str(),
            result["runtime_status"].as_str(),
            result["server_version"].as_str()
        ),
        (Some("ses_synthetic"), Some("idle"), Some("1.18.30"))
    );
    assert_eq!(
        (result["url"].as_str(), result["transport"].as_str()),
        (
            Some(c.fake.url.as_str()),
            Some("opencode_http_prompt_async")
        )
    );
    let requests = c.fake.requests();
    assert_eq!(
        requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        [
            "/global/health",
            "/session/ses_synthetic",
            "/session/status"
        ]
    );
    assert!(requests[0].query.is_empty());
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.query.get("directory") == Some(&c.workspace))
    );
    assert!(
        requests
            .iter()
            .all(|r| r.method == "GET" && r.body.is_empty())
    );

    c.fake
        .with(|s| s.status = json!({"ses_synthetic": {"type": "busy"}}));
    assert_eq!(opencode::probe(&c.peer).unwrap()["runtime_status"], "busy");
    c.fake
        .with(|s| s.status = json!({"ses_synthetic": null, "ses_other": {"type": "odd"}}));
    assert_eq!(opencode::probe(&c.peer).unwrap()["runtime_status"], "idle");
    c.fake
        .with(|s| s.status = json!({"ses_synthetic": {"type": "unrecognized"}}));
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_invalid_response");
    c.fake.with(|s| s.status = json!([]));
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_invalid_response");
    c.fake.with(|s| s.status = json!({}));

    c.fake
        .with(|s| s.sessions[0]["directory"] = json!(format!("{}/elsewhere", c.workspace)));
    assert_eq!(err(opencode::probe(&c.peer)), "recipient_identity_changed");
    c.fake.with(|s| s.sessions[0]["directory"] = json!(""));
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_invalid_response");
    let link = c._dir.path().join("link");
    std::os::unix::fs::symlink(&c.workspace, &link).unwrap();
    c.fake
        .with(|s| s.sessions[0]["directory"] = json!(link.to_str().unwrap()));
    assert_eq!(
        opencode::probe(&c.peer).unwrap()["workspace"].as_str(),
        Some(c.workspace.as_str())
    );
    c.fake
        .with(|s| s.sessions[0] = session("ses_synthetic", c.workspace.as_str()));
    c.fake
        .with(|s| s.sessions[0]["time"]["archived"] = json!(3));
    assert_eq!(err(opencode::probe(&c.peer)), "recipient_session_archived");
    c.fake
        .with(|s| s.sessions[0]["time"]["archived"] = Value::Null);
    assert!(opencode::probe(&c.peer).is_ok());
    c.fake.with(|s| s.sessions[0]["time"] = json!([]));
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_invalid_response");
    c.fake.with(|s| s.sessions = vec![]);
    assert_eq!(
        err(opencode::probe(&c.peer)),
        "recipient_not_in_opencode_server"
    );
}

#[test]
fn responses_follow_http_client_framing() {
    let c = case();
    let body = r#"{"healthy": true, "version": "framed"}"#;
    let responses = [
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{}\r\n{:x};ext=1\r\n{}\r\n0\r\nTrailer: x\r\n\r\n",
            &body[..5],
            body.len() - 5,
            &body[5..]
        ),
        format!("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.0 200 OK\r\n\r\n{body}"), // no length: read to close
        format!("HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{body}"), // short body, as read(amt)
        format!(
            "HTTP/1.1  200\r\ncontent-length:{}\r\n\r\n\u{feff}{body}",
            body.len() + 3
        ),
        format!("HTTP/1.1 200 OK\r\nContent-Length: nonsense\r\n\r\n{body}"),
    ];
    for response in responses {
        health_hook(&c, move || raw(&response));
        assert_eq!(
            opencode::probe(&c.peer).unwrap()["server_version"],
            "framed"
        );
    }
}

#[test]
fn response_bound_is_four_mebibytes() {
    let c = case();
    let json_of = |size: usize| {
        let base = r#"{"healthy": true, "version": ""}"#;
        format!(
            r#"{{"healthy": true, "version": "{}"}}"#,
            "v".repeat(size - base.len())
        )
    };
    let exact = json_of(opencode::MAX_RESPONSE);
    health_hook(&c, move || {
        raw(&format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{exact}",
            exact.len()
        ))
    });
    assert_eq!(
        opencode::probe(&c.peer).unwrap()["server_version"]
            .as_str()
            .unwrap()
            .len(),
        opencode::MAX_RESPONSE - 32
    );
    let over = json_of(opencode::MAX_RESPONSE + 1);
    health_hook(&c, move || raw(&format!("HTTP/1.1 200 OK\r\n\r\n{over}")));
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_response_too_large");
    let chunk = "v".repeat(opencode::MAX_RESPONSE + 10);
    health_hook(&c, move || {
        raw(&format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n0\r\n\r\n",
            chunk.len()
        ))
    });
    assert_eq!(err(opencode::probe(&c.peer)), "opencode_response_too_large");
}

#[test]
fn prompt_status_classification() {
    let c = case();
    for (status, expected) in [
        (204, ("submitted", "opencode_prompt_async_accepted")),
        (404, ("not_submitted", "recipient_not_in_opencode_server")),
        (401, ("not_submitted", "opencode_unauthorized")),
        (409, ("not_submitted", "opencode_http_409")),
        (400, ("not_submitted", "opencode_http_400")),
        (500, ("submission_unknown", "opencode_http_500")),
        (200, ("submission_unknown", "opencode_http_200")),
        (302, ("submission_unknown", "opencode_http_302")),
    ] {
        c.fake.with(|s| s.prompt_status = status);
        assert_eq!(
            outcome(opencode::notify(&c.peer, "text", &no_skip)),
            (expected.0, expected.1.to_owned()),
            "{status}"
        );
    }
    assert_eq!(c.fake.posts().len(), 8);
}

#[test]
fn failures_after_the_request_is_written_are_uncertain() {
    let c = case();
    let post = |reply: fn() -> Reply| {
        c.fake.hook(move |r| (r.method == "POST").then(reply));
        let before = c.fake.posts().len();
        let result = outcome(opencode::notify(&c.peer, "text", &no_skip));
        assert_eq!(c.fake.posts().len(), before + 1);
        result
    };
    assert_eq!(
        post(|| Reply::Close),
        ("submission_unknown", "opencode_RemoteDisconnected".into())
    );
    assert_eq!(
        post(|| raw("HTTP/1.1 400 Bad\r\nContent-Length: 3\r\n\r\n{x}")),
        ("submission_unknown", "opencode_invalid_response".into())
    );
    assert_eq!(
        post(|| raw("HTTP/1.1 2x4 odd\r\n\r\n")),
        ("submission_unknown", "opencode_BadStatusLine".into())
    );
    assert_eq!(
        post(|| raw("HTTP/1.1 204 No Content\r\n\r\n")),
        ("submitted", "opencode_prompt_async_accepted".into())
    );
    assert_eq!(
        post(|| Reply::Sleep(Duration::from_secs(7))),
        ("submission_unknown", "opencode_TimeoutError".into())
    );
}

#[test]
fn preflight_failures_never_post() {
    let c = case();
    c.fake.with(|s| s.sessions.clear());
    assert_eq!(
        outcome(opencode::notify(&c.peer, "text", &no_skip)),
        ("not_submitted", "recipient_not_in_opencode_server".into())
    );
    let peer = Peer {
        url: Some(format!("http://127.0.0.1:{}", closed_port())),
        ..c.peer.clone()
    };
    assert_eq!(
        outcome(opencode::notify(&peer, "text", &no_skip)),
        ("not_submitted", "opencode_unreachable".into())
    );
    let peer = Peer {
        url: Some("http://example.invalid:4096".into()),
        ..c.peer.clone()
    };
    assert_eq!(
        outcome(opencode::notify(&peer, "text", &no_skip)),
        ("not_submitted", "opencode_url_must_be_loopback".into())
    );
    c.fake
        .with(|s| s.sessions = vec![session("ses_synthetic", "/synthetic/elsewhere")]);
    assert_eq!(
        outcome(opencode::notify(&c.peer, "text", &no_skip)),
        ("not_submitted", "recipient_identity_changed".into())
    );
    // A preflight transport failure after its GET was written is still not a submission.
    c.fake
        .with(|s| s.sessions = vec![session("ses_synthetic", c.workspace.as_str())]);
    health_hook(&c, || Reply::Close);
    assert_eq!(
        outcome(opencode::notify(&c.peer, "text", &no_skip)),
        ("not_submitted", "opencode_RemoteDisconnected".into())
    );
    assert!(c.fake.posts().is_empty());
}

#[test]
fn saved_state_is_rechecked_after_preflight_and_before_the_post() {
    let c = case();
    for (reason, detail) in [
        (
            SkipReason::AcknowledgedBeforeNotification,
            "acknowledged_before_notification",
        ),
        (
            SkipReason::ReturnedByRecipientWait,
            "returned_by_recipient_wait",
        ),
        (SkipReason::RecipientRetired, "recipient_retired"),
    ] {
        let seen = Cell::new(0);
        let skip = || {
            seen.set(c.fake.requests().len());
            Ok(Some(reason))
        };
        assert_eq!(
            outcome(opencode::notify(&c.peer, "text", &skip)),
            ("not_submitted", detail.into())
        );
        assert_eq!(seen.get() % 3, 0);
        assert!(seen.get() >= 3); // Called after the three preflight requests.
    }
    let skip = || Err(Failure::coded("notification_state_unavailable"));
    assert_eq!(
        outcome(opencode::notify(&c.peer, "text", &skip)),
        ("not_submitted", "notification_state_unavailable".into())
    );
    let skip = || Err(Failure::Class("OperationalError"));
    assert_eq!(
        outcome(opencode::notify(&c.peer, "text", &skip)),
        ("not_submitted", "OperationalError".into())
    );
    // Preflight failure: the saved state is not even read.
    let called = Cell::new(false);
    let skip = || {
        called.set(true);
        Ok(None)
    };
    c.fake.with(|s| s.sessions.clear());
    opencode::notify(&c.peer, "text", &skip);
    assert!(!called.get());
    assert!(c.fake.posts().is_empty());
}

#[test]
fn failed_tls_handshake_is_before_the_request() {
    // A plain-HTTP listener answering an https URL: the handshake fails before any request is written.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut first = [0u8; 5];
        let _ = stream.read(&mut first);
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        first
    });
    let dir = fixture::tempdir();
    let peer = Peer {
        name: "muse".into(),
        harness: Harness::Opencode,
        session_id: "ses_synthetic".into(),
        workspace: dir.path().to_str().unwrap().into(),
        socket: None,
        url: Some(format!("https://127.0.0.1:{port}")),
        retired_at: None,
    };
    assert_eq!(
        outcome(opencode::notify(&peer, "text", &no_skip)),
        ("not_submitted", "opencode_unreachable".into())
    );
    assert_eq!(server.join().unwrap()[0], 0x16); // A TLS handshake record, not an HTTP request line.
}
