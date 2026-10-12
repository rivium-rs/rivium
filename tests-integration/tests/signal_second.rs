//! A second signal aborts a stop that has not finished; a normal exit is not accepted.
#![cfg(unix)]

use std::time::{Duration, Instant};

use rivium_test::process::spawn;
use tests_integration::{send_signal, test_binary, test_root};

#[test]
#[cfg_attr(qemu, ignore = "requires native Unix signal delivery")]
fn a_second_signal_aborts_with_its_own_number() {
    for (signal, name, code) in [
        (libc::SIGTERM, "SIGTERM", 143),
        (libc::SIGINT, "SIGINT", 130),
    ] {
        let root = test_root();
        let mut args = root.args().to_vec();
        // Leave time for the second signal even on a busy runner; the test still requires a quick abort.
        args.extend(["--set", "lifecycle.stop_timeout=10s"]);
        let proc = spawn(&test_binary("host_second_signal"), &args);
        proc.wait_for("phase changed", &[("phase", "running")]);
        send_signal(proc.id(), libc::SIGTERM).expect("send first SIGTERM");
        proc.wait_for(
            "phase changed",
            &[("phase", "stopping"), ("reason", "SIGTERM")],
        );

        let aborting = Instant::now();
        send_signal(proc.id(), signal).expect("send second signal while stopping");
        let exited = proc.wait();
        assert_eq!(exited.code, Some(code), "second {name}: {}", exited.stderr);
        assert!(aborting.elapsed() < Duration::from_secs(5));
        assert!(
            exited
                .stderr
                .contains(&format!("second stop request ({name})")),
            "{}",
            exited.stderr
        );
    }
}
