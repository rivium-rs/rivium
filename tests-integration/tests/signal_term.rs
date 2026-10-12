//! SIGTERM stops normally and flushes each lifecycle event once.
#![cfg(unix)]

use std::time::{Duration, Instant};

use rivium_test::process::spawn;
use tests_integration::{send_signal, test_binary, test_root};

#[test]
#[cfg_attr(qemu, ignore = "requires native Unix signal delivery")]
fn sigterm_stops_with_0_and_flushes_the_log() {
    let root = test_root();
    let mut args = root.args().to_vec();
    args.extend(["--set", "log.file.filter=info"]);
    let proc = spawn(&test_binary("host"), &args);
    proc.wait_for("phase changed", &[("phase", "running")]);

    let stopping = Instant::now();
    send_signal(proc.id(), libc::SIGTERM).expect("send SIGTERM");
    let exited = proc.wait();
    assert_eq!((exited.code, exited.stderr.as_str()), (Some(0), ""));
    assert!(stopping.elapsed() < Duration::from_secs(5));

    let log = root.log("test-host");
    for event in [
        "rivium::process: starting ",
        "phase changed phase=\"running\"",
        "phase changed phase=\"stopping\" reason=SIGTERM",
        "outcome outcome=\"stopped\"",
        "rivium::process: stopped code=\"Ok\" exit_code=0",
    ] {
        assert_eq!(
            log.lines().filter(|line| line.contains(event)).count(),
            1,
            "{event}:\n{log}"
        );
    }
    assert!(
        log.lines()
            .last()
            .unwrap()
            .contains("rivium::process: stopped code=\"Ok\" exit_code=0"),
        "the final stop event must be flushed before exit:\n{log}"
    );
}
