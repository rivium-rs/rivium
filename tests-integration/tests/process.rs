//! Basic process tests: start, stop, exit codes.

use std::time::Duration;
use tests_integration::TestProcess;

#[test]
fn host_starts_and_stops() {
    let mut proc = TestProcess::spawn("host", &[]).expect("failed to spawn host");

    // Give it time to start.
    std::thread::sleep(Duration::from_millis(100));

    // Send SIGTERM.
    #[cfg(unix)]
    proc.signal(libc::SIGTERM).expect("failed to send SIGTERM");

    // Wait for clean exit.
    let status = proc
        .wait_timeout(Duration::from_secs(5))
        .expect("wait failed")
        .expect("process did not exit");

    assert!(status.success(), "host did not exit with 0");
}

#[test]
fn host_fault_build_exits_73() {
    let output = std::process::Command::new(tests_integration::test_binary("host_fault_build"))
        .output()
        .expect("failed to run host-fault-build");

    #[cfg(unix)]
    {
        assert_eq!(
            output.status.code(),
            Some(69),
            "expected exit code 69 (StartupFailed)"
        );
    }
}

#[test]
fn host_fault_ready_exits_69() {
    let output = std::process::Command::new(tests_integration::test_binary("host_fault_ready"))
        .output()
        .expect("failed to run host-fault-ready");

    #[cfg(unix)]
    {
        assert_eq!(
            output.status.code(),
            Some(69),
            "expected exit code 69 (StartupFailed before ready)"
        );
    }
}

#[test]
fn host_fault_run_exits_70() {
    let output = std::process::Command::new(tests_integration::test_binary("host_fault_run"))
        .output()
        .expect("failed to run host-fault-run");

    #[cfg(unix)]
    {
        assert_eq!(
            output.status.code(),
            Some(70),
            "expected exit code 70 (Fault after ready)"
        );
    }
}

#[test]
fn host_deadline_killed_after_timeout() {
    let mut proc = TestProcess::spawn("host_deadline", &[]).expect("failed to spawn");

    std::thread::sleep(Duration::from_millis(100));

    #[cfg(unix)]
    {
        proc.signal(libc::SIGTERM).expect("failed to send SIGTERM");

        // Default stop_timeout is 3s. Wait for deadline to trigger.
        let status = proc
            .wait_timeout(Duration::from_secs(5))
            .expect("wait failed")
            .expect("process did not exit");

        assert_eq!(
            status.code(),
            Some(124),
            "expected exit code 124 (StopTimedOut)"
        );
    }
}
