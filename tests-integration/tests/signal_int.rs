//! SIGINT requests the same graceful stop as SIGTERM.
#![cfg(unix)]

use std::time::{Duration, Instant};

use rivium_test::process::spawn;
use tests_integration::{send_signal, test_binary, test_root};

#[test]
#[cfg_attr(qemu, ignore = "requires native Unix signal delivery")]
fn sigint_stops_with_0() {
    let root = test_root();
    let proc = spawn(&test_binary("host_sigint"), &root.args());
    proc.wait_for("phase changed", &[("phase", "running")]);

    let stopping = Instant::now();
    send_signal(proc.id(), libc::SIGINT).expect("send SIGINT");
    proc.wait_for(
        "phase changed",
        &[("phase", "stopping"), ("reason", "SIGINT")],
    );
    let exited = proc.wait();
    assert_eq!((exited.code, exited.stderr.as_str()), (Some(0), ""));
    assert!(stopping.elapsed() < Duration::from_secs(5));
}
