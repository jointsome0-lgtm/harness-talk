//! What holds of a process group this program started, on every system that owns one.
#![cfg(all(mcp_server, unix))]
use harness_talk::os::OwnedGroup;
use std::time::{Duration, Instant};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn retained_tokio_wrapper_is_not_reaped_by_the_signal_driver() {
    runtime().block_on(async {
        let mut child = tokio::process::Command::new("true")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut group = OwnedGroup::new(child.id().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !group.exited().unwrap() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        group.check_anchor().unwrap();
        group.finish_async().await.unwrap();
        assert!(child.wait().await.unwrap().success());
        assert!(group.step(libc::SIGTERM).is_err());
    });
}

#[test]
fn a_member_that_outlives_the_wrapper_is_stopped_and_a_group_already_gone_is_owned() {
    runtime().block_on(async {
        // The wrapper exits at once and leaves a member of its group running.
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 >/dev/null & echo $!"])
            .stdout(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let leader = child.id().unwrap();
        let mut group = OwnedGroup::new(leader).unwrap();
        let mut printed = String::new();
        tokio::io::AsyncReadExt::read_to_string(child.stdout.as_mut().unwrap(), &mut printed)
            .await
            .unwrap();
        let member: i32 = printed.trim().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !group.exited().unwrap() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(0, unsafe { libc::kill(member, 0) }, "the member runs");
        assert!(!group.step(0).unwrap(), "the group still has a member");
        group.finish_async().await.unwrap();
        assert!(group.step(0).unwrap(), "the member was left running");
        assert!(child.wait().await.unwrap().success());

        // A wrapper that has exited before anyone asks is owned all the same.
        let mut child = tokio::process::Command::new("true")
            .process_group(0)
            .spawn()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut group = OwnedGroup::new(child.id().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !group.exited().unwrap() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        group.finish_async().await.unwrap();
        assert!(child.wait().await.unwrap().success());
    });
}

#[test]
fn a_wrapper_on_its_way_out_is_no_error_and_its_group_is_not_yet_empty() {
    runtime().block_on(async {
        // A wrapper that was told to end is, for a moment, neither signalled any more nor said
        // to have exited: macOS showed that moment last for milliseconds. The group is asked
        // without a pause, so that some question falls in it.
        for _ in 0..10 {
            let mut child = tokio::process::Command::new("/bin/sh")
                .args(["-c", "echo started; exec sleep 30"])
                .stdout(std::process::Stdio::piped())
                .process_group(0)
                .spawn()
                .unwrap();
            let leader = child.id().unwrap();
            let mut group = OwnedGroup::new(leader).unwrap();
            let mut line = [0; 8];
            tokio::io::AsyncReadExt::read_exact(child.stdout.as_mut().unwrap(), &mut line)
                .await
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                // Whose group it is can be asked anew there as well.
                OwnedGroup::new(leader).unwrap();
                if group.step(libc::SIGKILL).unwrap() {
                    break;
                }
                assert!(Instant::now() < deadline);
            }
            assert!(group.exited().unwrap(), "empty while its wrapper ran");
            child.wait().await.unwrap();
        }
    });
}
