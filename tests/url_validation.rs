use harness_talk::validate::opencode_url;

#[test]
fn only_literal_loopback_hosts_are_accepted_before_url_normalization() {
    for host in [
        "127.1",
        "2130706433",
        "127.0.0.%31",
        "ｌｏｃａｌｈｏｓｔ",
        "localhost.example",
        "192.168.1.1",
    ] {
        assert_eq!(
            opencode_url(Some(&format!("http://{host}:4096")))
                .unwrap_err()
                .to_string(),
            "opencode_url_must_be_loopback"
        );
    }
    for url in [
        "http://127.0.0.1:4096\\path",
        "http://127.0.0.1:99999",
        "http://user@localhost",
        "http://localhost/?x",
    ] {
        assert!(opencode_url(Some(url)).is_err(), "{url}");
    }
    for (url, expected) in [
        ("HTTP://LOCALHOST:80/a/", "http://LOCALHOST:80/a"),
        ("http://[::1]:4096/", "http://[::1]:4096"),
        (" http://127.0.0.1:4096/a\tb", "http://127.0.0.1:4096/ab"),
    ] {
        assert_eq!(opencode_url(Some(url)).unwrap(), expected);
    }
}
