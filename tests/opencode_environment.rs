//! Environment-dependent OpenCode behavior: Basic auth, ignored proxies, the XDG metadata
//! default and verified HTTPS. One test, because it changes this process's environment.
#[path = "opencode_fixture.rs"]
mod fixture;

use fixture::{Fake, session};
use harness_talk::{error::Failure, model::*, opencode};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

const CA: &str = "-----BEGIN CERTIFICATE-----
MIIBqzCCAVGgAwIBAgIUVPwnIyBgbkio+AdeII1Lh+3zp1wwCgYIKoZIzj0EAwIw
IjEgMB4GA1UEAwwXaHRhbGsgc3ludGhldGljIHRlc3QgQ0EwIBcNMjYwOTIxMTcz
MDQ2WhgPMjEyNjA4MjgxNzMwNDZaMCIxIDAeBgNVBAMMF2h0YWxrIHN5bnRoZXRp
YyB0ZXN0IENBMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAElbDWoPA9on7fodE3
nrLqvR1XB2222ypSrmnubHniReHlVWNeNla4HI9bLmgfcR9svd2V2M6vzXQpQBCd
xQFotaNjMGEwHQYDVR0OBBYEFAt9N837vjOd25kuZEdD2VQOEcOzMB8GA1UdIwQY
MBaAFAt9N837vjOd25kuZEdD2VQOEcOzMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0P
AQH/BAQDAgIEMAoGCCqGSM49BAMCA0gAMEUCIBxK62Zvu5R4ZRTHKcZMRtZwRm/8
QOaA8DLbFozQ2gMlAiEA2COOE3SHaBLN4zNyHCWbAyGvvmqIPtNmF4COtw3aO/I=
-----END CERTIFICATE-----
";
/// A synthetic leaf for IP 127.0.0.1 and localhost, signed by CA; generated for this test only.
const LEAF: &str = "-----BEGIN CERTIFICATE-----
MIIBxzCCAW2gAwIBAgIUMI4Zcrd6BsE6O4vGA96CXRGDnIAwCgYIKoZIzj0EAwIw
IjEgMB4GA1UEAwwXaHRhbGsgc3ludGhldGljIHRlc3QgQ0EwIBcNMjYwOTIxMTcz
MDQ2WhgPMjEyNjA4MjgxNzMwNDZaMBQxEjAQBgNVBAMMCTEyNy4wLjAuMTBZMBMG
ByqGSM49AgEGCCqGSM49AwEHA0IABEBRyeGFVaxpQyqA2KeQAJIr1lKd0EPdP6wO
wVdn8Yaw117Mhrlwt2J3cTGwUTMMwjnQ9akPuDwlqrxbgy9k6/ijgYwwgYkwCQYD
VR0TBAIwADALBgNVHQ8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwGgYDVR0R
BBMwEYcEfwAAAYIJbG9jYWxob3N0MB0GA1UdDgQWBBTISAN2ur0Cjr8kSnqnyhyC
EUS5hTAfBgNVHSMEGDAWgBQLfTfN+74znduZLmRHQ9lUDhHDszAKBggqhkjOPQQD
AgNIADBFAiEA3DJlTfHYql2pUKhglQ1UXC8gOO6IUEy0nG0gWI4t0JICICmOTQvH
nlzPVoethsdRNpLe8hbmBiBhAb/puIdPLe9m
-----END CERTIFICATE-----
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgDe5UrX8K+Ui2gTxm
r1HcvURuuoazi+8b6BwiwopsSfChRANCAARAUcnhhVWsaUMqgNinkACSK9ZSndBD
3T+sDsFXZ/GGsNdezIa5cLdid3ExsFEzDMI50PWpD7g8Jaq8W4MvZOv4
-----END PRIVATE KEY-----
";
/// A synthetic HTTPS OpenCode answering health, one session and status.
const TLS_SERVER: &str = r#"
import http.server, json, ssl, sys
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        path = self.path.split("?")[0]
        data = {"/global/health": {"healthy": True, "version": "tls"}, "/session/status": {},
                "/session/ses_tls": {"id": "ses_tls", "directory": sys.argv[2], "time": {"updated": 1}}}.get(path)
        body = json.dumps(data).encode()
        self.send_response(200 if data is not None else 404)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(sys.argv[1])
server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_address[1], flush=True)
server.serve_forever()
"#;

fn set(name: &str, value: impl AsRef<std::ffi::OsStr>) {
    unsafe { std::env::set_var(name, value) }
}
fn unset(name: &str) {
    unsafe { std::env::remove_var(name) }
}
fn no_skip() -> Result<Option<SkipReason>, Failure> {
    Ok(None)
}

#[test]
fn environment_controls_credentials_metadata_and_trust_only() {
    let dir = fixture::tempdir();
    let ws = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    for name in [
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_SERVER_USERNAME",
        "SSL_CERT_DIR",
    ] {
        unset(name);
    }

    // Proxies in the environment are never used.
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://127.0.0.1:{}", proxy.local_addr().unwrap().port());
    let proxied = Arc::new(AtomicUsize::new(0));
    let count = proxied.clone();
    thread::spawn(move || {
        for _ in proxy.incoming() {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        set(name, &proxy_url);
    }
    unset("NO_PROXY");
    unset("no_proxy");

    let fake = Fake::start(vec![session("ses_synthetic", ws.as_str())]);
    let peer = Peer {
        name: "muse".into(),
        harness: Harness::Opencode,
        session_id: "ses_synthetic".into(),
        workspace: ws.clone(),
        socket: None,
        url: Some(fake.url.clone()),
        retired_at: None,
    };
    let result = opencode::probe(&peer).unwrap();
    assert_eq!(result["authenticated"], false);
    assert!(
        fake.requests()
            .iter()
            .all(|r| r.header("authorization").is_none())
    );

    fake.with(|s| s.password = Some("synthetic-secret".into()));
    assert_eq!(
        opencode::probe(&peer).unwrap_err().to_string(),
        "opencode_unauthorized"
    );
    let absent = dir.path().join("absent.db");
    let found = opencode::discover_with(Some(std::slice::from_ref(&fake.url)), None, Some(&absent));
    assert_eq!(found.sources[0]["error"], "opencode_unauthorized");
    set("OPENCODE_SERVER_PASSWORD", "synthetic-secret");
    set("OPENCODE_SERVER_USERNAME", "");
    let result = opencode::probe(&peer).unwrap();
    assert_eq!(result["authenticated"], true);
    assert!(!result.to_string().contains("synthetic-secret"));
    let outcome = opencode::notify(&peer, "text", &no_skip);
    assert_eq!(
        (outcome.submission, outcome.detail.as_str()),
        (Submission::Submitted, "opencode_prompt_async_accepted")
    );
    assert_eq!(
        fake.posts()[0].header("authorization"),
        Some("Basic b3BlbmNvZGU6c3ludGhldGljLXNlY3JldA==")
    );
    set("OPENCODE_SERVER_USERNAME", "other");
    assert_eq!(
        opencode::probe(&peer).unwrap_err().to_string(),
        "opencode_unauthorized"
    );
    assert_eq!(
        fake.requests().last().unwrap().header("authorization"),
        Some("Basic b3RoZXI6c3ludGhldGljLXNlY3JldA==")
    );
    set("OPENCODE_SERVER_PASSWORD", "");
    assert_eq!(
        opencode::probe(&peer).unwrap_err().to_string(),
        "opencode_unauthorized"
    );
    assert!(
        fake.requests()
            .last()
            .unwrap()
            .header("authorization")
            .is_none()
    );
    unset("OPENCODE_SERVER_PASSWORD");
    unset("OPENCODE_SERVER_USERNAME");
    assert_eq!(proxied.load(Ordering::SeqCst), 0);

    // The saved metadata default follows XDG_DATA_HOME; the default URL is replaced by an empty list here.
    let data = dir.path().join("xdg");
    std::fs::create_dir_all(data.join("opencode")).unwrap();
    let db = rusqlite::Connection::open(data.join("opencode/opencode.db")).unwrap();
    db.execute_batch("CREATE TABLE session (id TEXT, parent_id TEXT, directory TEXT, time_updated INTEGER, time_archived INTEGER);
        INSERT INTO session VALUES ('ses_xdg', NULL, '/synthetic/xdg', 42000, NULL);").unwrap();
    set("XDG_DATA_HOME", &data);
    let found = opencode::discover(Some(&[]), None);
    assert_eq!(
        (
            found.sessions[0]["session_id"].as_str(),
            found.sources[0]["path"].as_str()
        ),
        (Some("ses_xdg"), data.join("opencode/opencode.db").to_str())
    );
    set("XDG_DATA_HOME", "");
    set("HOME", dir.path().join("home"));
    let found = opencode::discover(Some(&[]), None);
    assert_eq!(
        found.sources[0]["path"].as_str(),
        dir.path()
            .join("home/.local/share/opencode/opencode.db")
            .to_str()
    );
    assert_eq!(found.sources[0]["error"], "opencode_saved_metadata_missing");

    // HTTPS verifies the server against SSL_CERT_FILE, like Python's default context.
    let cert = dir.path().join("leaf.pem");
    std::fs::write(&cert, LEAF).unwrap();
    let Ok(mut child) = Command::new("/usr/bin/python3")
        .args(["-B", "-c", TLS_SERVER, cert.to_str().unwrap(), &ws])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        eprintln!("python3 unavailable; HTTPS success not checked");
        return;
    };
    let mut port = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut port)
        .unwrap();
    let tls_peer = Peer {
        session_id: "ses_tls".into(),
        url: Some(format!("https://127.0.0.1:{}", port.trim())),
        ..peer.clone()
    };
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, CA).unwrap();
    set("SSL_CERT_FILE", &ca);
    let checked = opencode::probe(&tls_peer);
    let localhost = Peer {
        url: Some(format!("https://localhost:{}", port.trim())),
        ..tls_peer.clone()
    };
    let by_name = opencode::probe(&localhost).map(|r| r["server_version"].clone());
    set("SSL_CERT_FILE", dir.path().join("none.pem"));
    let untrusted = opencode::probe(&tls_peer);
    let _ = child.kill();
    let mut rest = Vec::new();
    let _ = child.stdout.take().unwrap().read_to_end(&mut rest);
    let _ = child.wait();
    unset("SSL_CERT_FILE");
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        unset(name);
    }
    let checked = checked.unwrap();
    assert_eq!(
        (checked["server_version"].as_str(), checked["url"].as_str()),
        (Some("tls"), tls_peer.url.as_deref())
    );
    // Every resolved localhost address is tried, as socket.create_connection does.
    assert_eq!(by_name.unwrap(), json!("tls"));
    assert_eq!(untrusted.unwrap_err().to_string(), "opencode_unreachable");
    assert_eq!(proxied.load(Ordering::SeqCst), 0);
}
