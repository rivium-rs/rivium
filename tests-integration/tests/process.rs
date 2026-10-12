//! Basic process tests: start, stop, exit codes.

use rivium_test::process::spawn;
use tests_integration::{test_binary, test_root};

#[cfg(unix)]
use std::time::{Duration, Instant};
#[cfg(unix)]
use tests_integration::send_signal;

#[cfg(unix)]
#[test]
fn host_starts_and_stops() {
    let root = test_root();
    let proc = spawn(&test_binary("host"), &root.args());
    proc.wait_for("phase changed", &[("phase", "running")]);
    let stopping = Instant::now();
    send_signal(proc.id(), libc::SIGTERM).expect("send SIGTERM");
    assert_eq!(proc.wait().code, Some(0));
    assert!(stopping.elapsed() < Duration::from_secs(5));
}

#[test]
fn host_fault_build_exits_69() {
    let root = test_root();
    let proc = spawn(&test_binary("host_fault_build"), &root.args());
    assert_eq!(proc.wait().code, Some(69));
}

#[test]
fn host_fault_ready_exits_69() {
    let root = test_root();
    let proc = spawn(&test_binary("host_fault_ready"), &root.args());
    assert_eq!(proc.wait().code, Some(69));
}

#[test]
fn host_fault_run_exits_70() {
    let root = test_root();
    let proc = spawn(&test_binary("host_fault_run"), &root.args());
    assert_eq!(proc.wait().code, Some(70));
}

#[cfg(unix)]
#[test]
fn host_deadline_exits_124() {
    let root = test_root();
    let proc = spawn(&test_binary("host_deadline"), &root.args());
    proc.wait_for("phase changed", &[("phase", "running")]);
    let stopping = Instant::now();
    send_signal(proc.id(), libc::SIGTERM).expect("send SIGTERM");
    assert_eq!(proc.wait().code, Some(124));
    assert!(stopping.elapsed() < Duration::from_secs(5));
}
