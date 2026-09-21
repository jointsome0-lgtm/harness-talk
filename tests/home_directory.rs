use harness_talk::os;
use std::{env, path::Path, process::Command};

#[test]
fn home_matches_python_with_missing_empty_and_custom_environment() {
    // Run each case in a child so no test changes process-wide environment.
    // Resolving these paths must not open a real mailbox or client directory.
    if let Some(expected) = env::var_os("HTALK_HOME_EXPECTED") {
        let expected = Path::new(&expected);
        assert_eq!(os::home(), expected);
        assert_eq!(os::expand_user(Path::new("~")), expected);
        assert_eq!(
            os::expand_user(Path::new("~/.local/share/harness-talk")),
            expected.join(".local/share/harness-talk")
        );
        return;
    }
    for home in [
        None,
        Some(""),
        Some("/tmp/htalk-custom-home"),
        Some("relative-home"),
    ] {
        let mut python = Command::new("python3");
        python.args(["-B", "-c", "from pathlib import Path; print(Path.home())"]);
        let mut child = Command::new(env::current_exe().unwrap());
        child.args([
            "--exact",
            "home_matches_python_with_missing_empty_and_custom_environment",
        ]);
        for command in [&mut python, &mut child] {
            match home {
                Some(value) => {
                    command.env("HOME", value);
                }
                None => {
                    command.env_remove("HOME");
                }
            }
        }
        let reference = python.output().unwrap();
        assert!(reference.status.success());
        let expected = String::from_utf8(reference.stdout).unwrap();
        let result = child
            .env("HTALK_HOME_EXPECTED", expected.trim_end_matches('\n'))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "HOME={home:?}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
