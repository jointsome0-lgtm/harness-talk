//! Peer registration, immutable addresses and retirement.
#[path = "store_support.rs"]
mod support;

use harness_talk::{error::Error, model::*};
use std::{fs, os::unix::fs::symlink};
use support::*;

#[test]
fn address_is_immutable_and_unique() {
    let temp = Temp::new();
    let store = store(&temp, &["alice"]);
    let bob = store
        .add_peer(
            "bob",
            Harness::Claude,
            &new_id().to_uppercase(),
            temp.workspace(),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        bob.session_id,
        bob.session_id.as_ref().map(|s| s.to_lowercase())
    );
    // An identical registration is idempotent, in any accepted spelling.
    assert_eq!(
        bob,
        store
            .add_peer(
                "bob",
                Harness::Claude,
                &bob.session_id.as_deref().unwrap().to_uppercase(),
                temp.workspace(),
                None,
                None
            )
            .unwrap()
    );
    assert_eq!(
        "peer_already_has_a_different_address",
        code(store.add_peer(
            "bob",
            Harness::Claude,
            &new_id(),
            temp.workspace(),
            None,
            None
        ))
    );
    let elsewhere = temp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    assert_eq!(
        "peer_already_has_a_different_address",
        code(store.add_peer(
            "bob",
            Harness::Claude,
            bob.session_id.as_deref().unwrap(),
            elsewhere.to_str().unwrap(),
            None,
            None
        ))
    );
    assert_eq!(
        "session_already_has_a_peer_name",
        code(store.add_peer(
            "other",
            Harness::Claude,
            bob.session_id.as_deref().unwrap(),
            temp.workspace(),
            None,
            None
        ))
    );
    // The same session under another harness is a different address.
    store
        .add_peer(
            "other",
            Harness::Codex,
            bob.session_id.as_deref().unwrap(),
            temp.workspace(),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        Some(bob.clone()),
        store
            .session_peer("claude", bob.session_id.as_deref().unwrap())
            .unwrap()
    );
    assert_eq!(None, store.session_peer("claude", &new_id()).unwrap());
    assert_eq!(
        vec!["alice", "bob", "other"],
        store
            .peers()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn registration_validates_and_normalizes_each_field() {
    let temp = Temp::new();
    let store = store(&temp, &[]);
    let ws = temp.workspace();
    assert_eq!(
        "invalid_peer_name",
        code(store.add_peer("Bob", Harness::Claude, &new_id(), ws, None, None))
    );
    assert_eq!(
        "badly formed hexadecimal UUID string",
        code(store.add_peer("bob", Harness::Claude, "nope", ws, None, None))
    );
    assert_eq!(
        "url_is_only_for_opencode",
        code(store.add_peer(
            "bob",
            Harness::Codex,
            &new_id(),
            ws,
            None,
            Some("http://127.0.0.1:1")
        ))
    );
    assert_eq!(
        "claude_socket_is_discovered_from_live_identity",
        code(store.add_peer("bob", Harness::Claude, &new_id(), ws, Some("/x.sock"), None))
    );
    assert_eq!(
        "opencode_uses_a_server_url_not_a_socket",
        code(store.add_peer("oc", Harness::Opencode, "ses_a", ws, Some("/x.sock"), None))
    );
    assert_eq!(
        "invalid_opencode_session_id",
        code(store.add_peer("oc", Harness::Opencode, &new_id(), ws, None, None))
    );
    assert_eq!(
        "opencode_url_must_be_loopback",
        code(store.add_peer(
            "oc",
            Harness::Opencode,
            "ses_a",
            ws,
            None,
            Some("http://example.com")
        ))
    );
    let file = temp.path().join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        "workspace_must_be_a_directory",
        code(store.add_peer(
            "bob",
            Harness::Claude,
            &new_id(),
            file.to_str().unwrap(),
            None,
            None
        ))
    );
    let missing = store.add_peer(
        "bob",
        Harness::Claude,
        &new_id(),
        temp.path().join("missing").to_str().unwrap(),
        None,
        None,
    );
    assert!(matches!(missing, Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound));
    assert!(store.peers().unwrap().is_empty());

    let opencode = store
        .add_peer("oc", Harness::Opencode, "ses_a", ws, None, None)
        .unwrap();
    assert_eq!(
        (Some("http://127.0.0.1:4096"), None),
        (opencode.url.as_deref(), opencode.socket.as_deref())
    );
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    symlink(&project, temp.path().join("alias")).unwrap();
    let codex = store
        .add_peer(
            "cx",
            Harness::Codex,
            &new_id(),
            temp.path().join("alias").to_str().unwrap(),
            Some(temp.path().join("alias/../codex.sock").to_str().unwrap()),
            None,
        )
        .unwrap();
    assert_eq!(project.to_str(), codex.workspace.as_deref());
    assert_eq!(
        temp.path().join("codex.sock").to_str().unwrap(),
        codex.socket.unwrap()
    );
}

#[test]
fn every_peer_row_carries_retired_at() {
    let temp = Temp::new();
    let store = store(&temp, &["alice", "bob"]);
    let json = serde_json::to_value(store.peer("alice").unwrap()).unwrap();
    assert_eq!(Some(&serde_json::Value::Null), json.get("retired_at"));
    let first = store.retire("bob").unwrap().retired_at.unwrap();
    // Repeating keeps the first time; re-registering keeps the retirement.
    assert_eq!(Some(first), store.retire("bob").unwrap().retired_at);
    let bob = store.peer("bob").unwrap();
    assert_eq!(
        Some(first),
        store
            .add_peer(
                "bob",
                Harness::Claude,
                bob.session_id.as_deref().unwrap(),
                bob.workspace.as_deref().unwrap(),
                None,
                None
            )
            .unwrap()
            .retired_at
    );
    assert_eq!(
        Some(first),
        store
            .session_peer("claude", bob.session_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .retired_at
    );
    assert_eq!(
        vec![None, Some(first)],
        store
            .peers()
            .unwrap()
            .iter()
            .map(|p| p.retired_at)
            .collect::<Vec<_>>()
    );
    for _ in 0..2 {
        assert_eq!(None, store.restore("bob").unwrap().retired_at);
    }
    assert_eq!("unknown_peer", code(store.retire("carol")));
    assert_eq!("unknown_peer", code(store.restore("carol")));
    assert_eq!("unknown_peer", code(store.peer("carol")));
}
