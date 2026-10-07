//! Rules that hold before a signal is sent to a process group this program started.
//!
//! These tests reach inside. A caller cannot make a process identifier change hands or
//! make `/proc` return a damaged record, so no command shows what is checked here.
#![cfg(target_os = "linux")]
use harness_talk::os::{OwnedGroup, member_disappeared, parse_identity};
use std::{
    fs, io,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn reaped_owned_proc_entry_is_a_member_disappearance_but_not_a_valid_anchor() {
    use std::io::Read;
    let mut child = Command::new("/bin/true").process_group(0).spawn().unwrap();
    let group = OwnedGroup::new(child.id()).unwrap();
    let mut stat = fs::File::open(format!("/proc/{}/stat", child.id())).unwrap();
    child.wait().unwrap();
    let error = stat.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
    assert!(member_disappeared(&error));
    assert!(
        group.check_anchor().is_err(),
        "a disappeared anchor must fail closed"
    );
}

#[test]
fn member_disappearance_does_not_hide_permission_or_other_io_failures() {
    for errno in [libc::ENOENT, libc::ESRCH] {
        assert!(member_disappeared(&io::Error::from_raw_os_error(errno)));
    }
    for errno in [libc::EPERM, libc::EACCES, libc::EIO] {
        assert!(!member_disappeared(&io::Error::from_raw_os_error(errno)));
    }
    assert!(!member_disappeared(&io::Error::other(
        "Invalid process stat"
    )));
}

#[test]
fn high_process_handles_and_reaped_anchor_fail_closed() {
    let files: Vec<_> = (0..1100)
        .map(|_| fs::File::open("/dev/null").unwrap())
        .collect();
    let mut child = Command::new("/bin/true")
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    // With 1100 files open, the group's process handle is numbered above 1024.
    let group = OwnedGroup::new(child.id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !group.exited().unwrap() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(child.wait().unwrap().success());
    let mut group = group;
    assert!(group.step(libc::SIGKILL).is_err());
    drop(files);
}

#[test]
fn stat_identity_accepts_non_utf8_comm_with_embedded_delimiters() {
    // comm is arbitrary bytes and can include parentheses or whitespace.
    // The final ')' separates it from the kernel's ASCII numeric fields.
    let mut stat = b"123 (odd\xff) name\n) S 1 123".to_vec();
    for _ in 3..19 {
        stat.extend_from_slice(b" 0");
    }
    stat.extend_from_slice(b" 456 0 0\n");
    let found = parse_identity(&stat).unwrap();
    assert_eq!((found.group, found.start), (123, 456));
    assert!(parse_identity(b"123 (odd\xff) S 1").is_err());
    assert!(parse_identity(b"123 (odd\xff S 1 123").is_err());
    let mut malformed = stat.clone();
    malformed.truncate(malformed.len() - b"456 0 0\n".len());
    malformed.extend_from_slice(b"\xff 0 0\n");
    assert!(parse_identity(&malformed).is_err());
}

fn stat_with_identity(group: &[u8], start: &[u8]) -> Vec<u8> {
    // Kernel stat field layout, with the signed group/session defaults
    // available when do_task_stat cannot lock an exiting task's sighand.
    let fields: &[&[u8]] = &[
        b"X",  // state (3)
        b"0",  // ppid (4)
        group, // pgrp (5), signed
        b"-1", // session (6)
        b"0",  // tty_nr (7)
        b"-1", // tpgid (8)
        b"0",  // flags (9)
        b"0",  // minflt (10)
        b"0",  // cminflt (11)
        b"0",  // majflt (12)
        b"0",  // cmajflt (13)
        b"0",  // utime (14)
        b"0",  // stime (15)
        b"0",  // cutime (16)
        b"0",  // cstime (17)
        b"20", // priority (18)
        b"0",  // nice (19)
        b"0",  // num_threads (20)
        b"0",  // itrealvalue (21)
        start, // starttime (22), unsigned
    ];
    let mut stat = b"123 (exiting fixture)".to_vec();
    for field in fields {
        stat.push(b' ');
        stat.extend_from_slice(field);
    }
    for _ in 23..=52 {
        stat.extend_from_slice(b" 0");
    }
    stat.push(b'\n');
    stat
}

#[test]
fn stat_identity_accepts_signed_kernel_group_and_unsigned_starttime() {
    let found = parse_identity(&stat_with_identity(b"-1", b"456")).unwrap();
    assert_eq!((found.group, found.start), (-1, 456));
    for (group, start) in [(0, 0), (123, 456), (i32::MAX, u64::MAX)] {
        let found = parse_identity(&stat_with_identity(
            group.to_string().as_bytes(),
            start.to_string().as_bytes(),
        ))
        .unwrap();
        assert_eq!((found.group, found.start), (group, start));
    }
}

#[test]
fn stat_identity_rejects_invalid_fields_without_truncation_or_record_contents() {
    for group in [
        b"bad".as_slice(),
        b"\xff",
        b"2147483648",
        b"-2147483649",
        // An unchecked u64-to-i32 cast would turn this into group 123.
        b"4294967419",
    ] {
        let error = parse_identity(&stat_with_identity(group, b"456"))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("process stat pgrp:"));
        assert!(!error.to_string().contains("exiting fixture"));
        assert!(!member_disappeared(&error));
    }
    for start in [b"bad".as_slice(), b"\xff", b"-1", b"18446744073709551616"] {
        let error = parse_identity(&stat_with_identity(b"123", start))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("process stat starttime:"));
        assert!(!error.to_string().contains("exiting fixture"));
        assert!(!member_disappeared(&error));
    }
    for (stat, field) in [
        (b"123 (private comm) X 0".as_slice(), "pgrp"),
        (b"123 (private comm) X 0 123".as_slice(), "starttime"),
    ] {
        let error = parse_identity(stat).err().unwrap();
        assert_eq!(
            error.to_string(),
            format!("Incomplete process stat {field}")
        );
        assert!(!member_disappeared(&error));
    }
}

#[test]
fn changed_anchor_is_rejected_before_any_signal() {
    let mut child = Command::new("/bin/sleep")
        .arg("10")
        .process_group(0)
        .spawn()
        .unwrap();
    let mut group = OwnedGroup::new(child.id()).unwrap();
    group.identity.start = group.identity.start.wrapping_add(1);
    let rejected = group.step(libc::SIGKILL).is_err();
    let untouched = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(rejected);
    assert!(untouched, "ownership rejection still signalled the child");
}
