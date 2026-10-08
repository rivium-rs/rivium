//! Process level: spawns the smoke binary (as later process-level tests will spawn the examples),
//! checks exit-code propagation and, on Unix, graceful stop on SIGTERM. Not built for Android,
//! where services run embedded.
#![cfg(not(target_os = "android"))]

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_smoke");

/// A command that runs the smoke binary. Cross-compiled Unix tests run under qemu-user, which does
/// not emulate child processes, so the child is started through cargo's target runner
/// (`CARGO_TARGET_<TRIPLE>_RUNNER`, which execs qemu) when one is configured. Under wine, child
/// processes are already emulated, so Windows targets never use the runner.
fn smoke() -> Command {
    let triple = env!("SMOKE_TARGET").to_uppercase().replace(['-', '.'], "_");
    let runner = std::env::var(format!("CARGO_TARGET_{triple}_RUNNER")).ok();
    match runner.filter(|_| cfg!(unix)) {
        Some(runner) => {
            let mut parts = runner.split_whitespace();
            let mut command = Command::new(parts.next().expect("runner is not empty"));
            command.args(parts).arg(BIN);
            command
        }
        None => Command::new(BIN),
    }
}

#[test]
fn exit_code_is_propagated() {
    let status = smoke().args(["exit", "75"]).status().unwrap();
    assert_eq!(status.code(), Some(75));
}

#[cfg(unix)]
#[test]
fn sigterm_stops_gracefully() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let log = std::env::temp_dir().join(format!("smoke-sigterm-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut child = smoke()
        .args(["serve", "--log", log.to_str().unwrap()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.starts_with("ready"), "unexpected first line: {line:?}");

    let kill = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "smoke did not stop within 10s");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(
        log_text.contains("stop signal=SIGTERM") && log_text.contains("stopped"),
        "{log_text}"
    );
    let _ = std::fs::remove_file(&log);
}
