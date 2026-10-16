#![cfg(feature = "process")]

use rivium::{Kind, Result, Service};
use rivium_host::{App, Context};
use rivium_test::TempDir;
use serde::{Deserialize, Serialize};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    mode: String,
}

struct Fixture;

impl App for Fixture {
    const NAME: &'static str = "process-fixture";
    const VERSION: &'static str = "test";
    type Config = Config;

    fn services(config: &Config, context: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        if config.mode == "restart-backoff" {
            use std::io::Write;
            writeln!(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(context.paths().root().join("builds"))
                    .unwrap(),
                "generation"
            )
            .unwrap();
        }
        if config.mode == "build-panic" {
            panic!("private constructor failure");
        }

        let mode = config.mode.clone();
        Ok(vec![rivium::service(
            "worker",
            Kind::Frontline,
            move |context| async move {
                if mode == "startup-error" {
                    return Err(rivium::Error::internal("private startup failure"));
                }
                context.ready();
                match mode.as_str() {
                    "run-error" | "restart-backoff" => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        return Err(rivium::Error::internal("private run failure"));
                    }
                    "restart" => context.control().restart("test"),
                    _ => {}
                }
                context.stopped().await;
                if mode == "stop-hang" {
                    std::future::pending::<()>().await;
                }
                Ok(())
            },
        )])
    }
}

#[test]
fn process_child() {
    let Ok(root) = std::env::var("RIVIUM_HOST_FIXTURE_ROOT") else {
        return;
    };
    let mode = std::env::var("RIVIUM_HOST_FIXTURE_MODE").unwrap_or_default();
    let mut args = vec!["--root".into(), root.into()];
    if mode == "default-config" {
        println!("CONFIG-BEGIN");
        args.push("default-config".into());
    } else if mode == "invalid" {
        args.push("--unknown".into());
    } else {
        args.extend([
            "--set".into(),
            format!("mode={mode}").into(),
            "--set".into(),
            "lifecycle.stop_timeout=150ms".into(),
        ]);
    }
    if mode == "restart-backoff" {
        args.extend(["--set".into(), "lifecycle.restart=in-process".into()]);
    }

    let code = rivium_host::process::run_args::<Fixture>(args);
    std::process::exit(i32::from(code.exit()));
}

fn command(root: &TempDir, mode: &str) -> Command {
    let mut command = rivium_test::command(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process_child", "--nocapture"])
        .env("RIVIUM_HOST_FIXTURE_ROOT", root.path())
        .env("RIVIUM_HOST_FIXTURE_MODE", mode)
        .env_remove("RUST_LOG");
    command
}

#[test]
fn process_preserves_exit_codes_and_emits_parseable_defaults() {
    let root = TempDir::new().unwrap();
    for (mode, expected) in [
        ("invalid", 64),
        ("startup-error", 69),
        ("build-panic", 69),
        ("run-error", 70),
        ("restart", 75),
    ] {
        let output = command(&root, mode).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let fresh = TempDir::new().unwrap();
    let output = command(&fresh, "default-config").output().unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let text = output.split_once("CONFIG-BEGIN\n").unwrap().1;
    let document: toml::Value = toml::from_str(text).unwrap();
    assert!(document.get("lifecycle").is_some());
    assert_eq!(std::fs::read_dir(fresh.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
#[cfg_attr(qemu, ignore = "native Unix signal delivery is verified separately")]
fn process_handles_first_second_signals_and_shared_deadline() {
    for (mode, signals, expected) in [
        ("normal", vec!["TERM"], 0),
        ("normal", vec!["INT"], 0),
        ("stop-hang", vec!["TERM"], 124),
        ("stop-hang", vec!["TERM", "INT"], 130),
    ] {
        let root = TempDir::new().unwrap();
        let mut child = command(&root, mode)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_log(&mut child, &root, |text| text.contains("services running"));
        for signal in signals {
            send_signal(&child, signal);
            std::thread::sleep(Duration::from_millis(20));
        }

        let status = wait_for(&mut child, Duration::from_secs(4), |child| {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(expected), "{mode}");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(qemu, ignore = "native Unix signal delivery is verified separately")]
fn stop_during_restart_backoff_exits_successfully_without_another_generation() {
    let root = TempDir::new().unwrap();
    let mut child = command(&root, "restart-backoff")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_log(&mut child, &root, |text| {
        text.matches("service failed").count() >= 3
    });
    let builds = std::fs::read_to_string(root.path().join("builds")).unwrap();
    send_signal(&child, "TERM");
    let status = wait_for(&mut child, Duration::from_secs(2), |child| {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(root.path().join("builds")).unwrap(),
        builds
    );
}

#[cfg(unix)]
fn send_signal(child: &std::process::Child, signal: &str) {
    assert!(
        Command::new("kill")
            .args(["-s", signal, &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(unix)]
fn wait_for<T>(
    child: &mut std::process::Child,
    within: Duration,
    mut observed: impl FnMut(&mut std::process::Child) -> Option<T>,
) -> T {
    let end = Instant::now() + within;
    loop {
        if let Some(value) = observed(child) {
            return value;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child exceeded the observation deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_log(child: &mut std::process::Child, root: &TempDir, condition: impl Fn(&str) -> bool) {
    wait_for(child, Duration::from_secs(5), |child| {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before expected log record"
        );
        let text =
            std::fs::read_to_string(root.path().join("logs/process-fixture/process-fixture.log"))
                .unwrap_or_default();
        condition(&text).then_some(())
    });
}
