//! What holds of a process group this program started, on every system that owns one.
#![cfg(mcp_server)]
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
        let mut child = tokio::process::Command::new("/bin/true")
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
        let mut child = tokio::process::Command::new("/bin/true")
            .process_group(0)
            .spawn()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut group = OwnedGroup::new(child.id().unwrap()).unwrap();
        assert!(group.exited().unwrap());
        group.finish_async().await.unwrap();
        assert!(child.wait().await.unwrap().success());
    });
}
