//! SIGQUIT retains its default signal termination, not a numeric timeout exit.
#![cfg(unix)]

use std::fs::{self, File};
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use rivium_test::process::command;
use serde_json::Value;
use tests_integration::{send_signal, test_binary, test_root};

// Running::wait exposes only an exit code; this test needs the original signal status.
struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[allow(unsafe_code)]
fn without_core_dumps(command: &mut Command) {
    // SAFETY: the child hook only calls setrlimit and reads errno, without allocation or locks.
    // Changing the child's limit leaves the test runner's process-global state alone.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &limit) == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[test]
#[cfg_attr(qemu, ignore = "requires native Unix signal delivery")]
fn sigquit_terminates_by_signal_not_exit_code() {
    let root = test_root();
    let console = root.path().join("console.jsonl");
    let output = File::create(&console).expect("capture the child's output");
    let mut command = command(&test_binary("host_sigquit"));
    command
        .args(root.args())
        .args([
            "--set",
            "log.console.filter=info",
            "--set",
            "log.console.format=json",
        ])
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    without_core_dumps(&mut command);
    let mut child = Reap(command.spawn().expect("start host-sigquit"));

    let ready_by = Instant::now() + Duration::from_secs(10);
    loop {
        let bytes = fs::read(&console).unwrap();
        let output = String::from_utf8_lossy(&bytes);
        let ready = output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|event| event["message"] == "phase changed" && event["phase"] == "running");
        if ready {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "exited before ready:\n{output}"
        );
        assert!(Instant::now() < ready_by, "not ready within 10s:\n{output}");
        thread::sleep(Duration::from_millis(10));
    }

    send_signal(child.0.id(), libc::SIGQUIT).expect("send SIGQUIT");
    let exit_by = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < exit_by,
            "SIGQUIT did not terminate the child within 5s"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), None, "{status:?}");
    assert_eq!(status.signal(), Some(libc::SIGQUIT), "{status:?}");
    assert!(!root.path().join("core").exists());
}
