//! What every system gives, asked of the built executable: a mailbox between pull peers. And
//! where a part is not ported, its one answer. `build.rs` says which parts a system has.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

/// A temporary directory with a space and a letter outside ASCII in its name, removed on drop.
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let name = format!("htalk platform é {}", uuid::Uuid::new_v4());
        let path = std::env::temp_dir().join(name);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn htalk_bare(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_htalk"))
        .args(args)
        .env_remove("HTALK_DB")
        .env_remove("HTALK_PEER")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn run(db: &Path, args: &[&str]) -> Output {
    htalk_bare(&[&["--db", db.to_str().unwrap()], args].concat())
}

/// The exit code and the one JSON object a mailbox command prints.
fn htalk(db: &Path, args: &[&str]) -> (i32, Value) {
    let output = run(db, args);
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{args:?} printed no JSON: {:?} {:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap(), value)
}

#[test]
fn pull_peers_exchange_a_request_and_its_answer() {
    let temp = Temp::new();
    let db = temp.0.join("data").join("mail.sqlite3");
    for name in ["alice", "bob"] {
        let (code, peer) = htalk(
            &db,
            &[
                "peer",
                "add",
                name,
                "--harness",
                "generic",
                "--delivery",
                "pull",
            ],
        );
        assert_eq!((0, "pull"), (code, peer["delivery"].as_str().unwrap()));
    }
    assert!(db.is_file());

    let (code, sent) = htalk(
        &db,
        &["--as", "alice", "send", "bob", "--message", "Question"],
    );
    assert_eq!(0, code, "{sent}");
    assert_eq!(
        ("saved", "not_submitted", "pull_only"),
        (
            sent["state"].as_str().unwrap(),
            sent["submission"].as_str().unwrap(),
            sent["notification_detail"].as_str().unwrap()
        )
    );
    let id = sent["id"].as_str().unwrap();

    let (code, inbox) = htalk(&db, &["--as", "bob", "inbox"]);
    assert_eq!((0, 1), (code, inbox["messages"].as_array().unwrap().len()));
    let (code, shown) = htalk(&db, &["--as", "bob", "show", id]);
    assert_eq!(
        (0, "Question", "alice"),
        (
            code,
            shown["body"].as_str().unwrap(),
            shown["sender"].as_str().unwrap()
        )
    );
    assert!(shown["ack_at"].is_null());
    let (code, acked) = htalk(&db, &["--as", "bob", "ack", id]);
    assert_eq!(0, code, "{acked}");
    assert!(acked["ack_at"].is_number());

    let (code, reply) = htalk(&db, &["--as", "bob", "reply", id, "--message", "Answer"]);
    assert_eq!(
        (0, id),
        (code, reply["in_reply_to"].as_str().unwrap()),
        "{reply}"
    );
    let (code, request) = htalk(&db, &["--as", "alice", "show", id]);
    assert_eq!(
        (0, "Answer"),
        (code, request["reply"]["body"].as_str().unwrap())
    );
    let (_, inbox) = htalk(&db, &["--as", "bob", "inbox"]);
    assert!(inbox["messages"].as_array().unwrap().is_empty());
}

#[cfg(not(native_clients))]
#[test]
fn native_delivery_that_is_not_ported_answers_one_code_and_writes_nothing() {
    let temp = Temp::new();
    let db = temp.0.join("data").join("mail.sqlite3");
    let workspace = temp.0.to_str().unwrap();
    for harness in ["codex", "claude"] {
        let (code, refused) = htalk(
            &db,
            &[
                "peer",
                "add",
                "native",
                "--harness",
                harness,
                "--session",
                "6f0c1a52-3b7e-4d19-9c58-2e4a7b1d0f36",
                "--workspace",
                workspace,
            ],
        );
        assert_eq!(
            (2, "error", "unsupported_on_this_platform"),
            (
                code,
                refused["state"].as_str().unwrap(),
                refused["error"].as_str().unwrap()
            )
        );
        let hint = refused["next_action"].as_str().unwrap();
        assert!(hint.contains("--delivery pull"), "{hint}");
    }
    assert!(!temp.0.join("data").exists());

    // `receive` takes its mailbox from the remote watch, so no `--db`.
    let state = temp.0.join("state");
    let receive = htalk_bare(&["receive", "status", "--state", state.to_str().unwrap()]);
    assert_eq!(
        (Some(2), "htalk receive: unsupported_on_this_platform"),
        (
            receive.status.code(),
            String::from_utf8_lossy(&receive.stderr).trim()
        )
    );
    assert!(fs::read_dir(&temp.0).unwrap().next().is_none());
}

#[cfg(not(mcp_server))]
#[test]
fn the_mcp_server_that_is_not_ported_answers_one_code() {
    let temp = Temp::new();
    let db = temp.0.join("mail.sqlite3");
    let server = run(&db, &["--as", "alice", "mcp"]);
    assert_eq!(
        (Some(2), "htalk mcp: unsupported_on_this_platform"),
        (
            server.status.code(),
            String::from_utf8_lossy(&server.stderr).trim()
        )
    );
    assert!(!db.exists());
}

#[cfg(all(feature = "catalog", not(catalog)))]
#[test]
fn the_catalogue_that_is_not_ported_answers_one_code() {
    let temp = Temp::new();
    let db = temp.0.join("mail.sqlite3");
    for args in [
        &["catalog"][..],
        &["catalog", "list"],
        &["catalog", "serve", "--stdio"],
    ] {
        let (code, refused) = htalk(&db, args);
        assert_eq!(
            (2, "unsupported_on_this_platform"),
            (code, refused["error"].as_str().unwrap()),
            "{args:?}"
        );
    }
    assert!(!db.exists());
}
