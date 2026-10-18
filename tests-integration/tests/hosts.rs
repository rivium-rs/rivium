use rivium_host::{
    Code,
    embedded::{Host, Status},
};
use rivium_test::TempDir;
use std::{sync::Arc, time::Duration};
use test_app::TestApp;

fn args(dir: &TempDir, mode: &str) -> Vec<String> {
    vec![
        "--root".into(),
        dir.path().display().to_string(),
        "--set".into(),
        format!("mode={mode}"),
    ]
}

#[test]
fn embedded_repeated_start_stop() {
    let dir = TempDir::new().unwrap();
    let host = Host::new::<TestApp>();
    assert_eq!(host.stop(Duration::from_secs(2)), Code::NotRunning);
    for _ in 0..3 {
        assert_eq!(host.start(&args(&dir, "normal")), Code::Ok);
        assert_eq!(host.status(), Status::Running);
        assert_eq!(host.start(&args(&dir, "normal")), Code::AlreadyRunning);
        assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
        assert_eq!(host.status(), Status::Idle);
    }
}

#[test]
fn embedded_startup_failures() {
    for mode in ["startup-error", "build-error", "build-panic"] {
        let dir = TempDir::new().unwrap();
        let host = Host::new::<TestApp>();
        assert_eq!(host.start(&args(&dir, mode)), Code::StartupFailed);
        assert!(host.last_error().is_some());
    }
}

#[test]
fn stop_during_start_returns_that_runs_result() {
    let dir = TempDir::new().unwrap();
    let host = Arc::new(Host::new::<TestApp>());
    let start_args = args(&dir, "startup-hang");
    let started = host.clone();
    let join = std::thread::spawn(move || started.start(&start_args));
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while host.status() == Status::Idle {
        assert!(std::time::Instant::now() < end);
        std::thread::yield_now();
    }
    let code = host.stop(Duration::from_secs(2));
    assert!(matches!(
        code,
        Code::Cancelled | Code::StopTimedOut | Code::Ok
    ));
    let first = join.join().unwrap();
    assert!(matches!(first, Code::Cancelled | Code::StopTimedOut));
}

#[test]
fn embedded_does_not_read_environment() {
    let dir = TempDir::new().unwrap();
    let options = rivium_host::Options::parse(
        "rivium-demo",
        args(&dir, "normal").into_iter().map(Into::into),
        false,
    )
    .unwrap();
    assert!(options.environment.is_empty());
}

#[test]
fn config_commands_are_read_only() {
    let dir = TempDir::new().unwrap();

    for command in ["--version", "--help", "check-config", "default-config"] {
        let output = tests_integration::command()
            .args([command, "--root"])
            .arg(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn process_errors_have_stable_codes() {
    let dir = TempDir::new().unwrap();

    for (extra, code) in [
        (vec!["--wrong"], 64),
        (vec!["--set", "mode=startup-error"], 69),
        (vec!["--set", "mode=build-panic"], 69),
        (vec!["--set", "mode=build-error"], 69),
        (vec!["--set", "mode=run-error"], 70),
        (vec!["--set", "mode=restart"], 75),
        (vec!["--set", "unknown=1"], 78),
    ] {
        let output = tests_integration::command()
            .arg("--root")
            .arg(dir.path())
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[cfg_attr(qemu, ignore = "native signal delivery is verified separately")]
#[test]
fn process_signals_and_deadline() {
    use std::process::{Command, Stdio};

    for (mode, signals, expected) in [
        ("normal", vec!["TERM"], 0),
        ("normal", vec!["INT"], 0),
        ("stop-hang", vec!["TERM"], 124),
        ("stop-hang", vec!["TERM", "INT"], 130),
        ("stop-hang", vec!["TERM", "TERM"], 143),
    ] {
        let dir = TempDir::new().unwrap();
        let timeout = if signals.len() > 1 {
            "lifecycle.stop_timeout=10s"
        } else {
            "lifecycle.stop_timeout=200ms"
        };
        let mut child = tests_integration::command()
            .env_remove("RUST_LOG")
            .args(args(&dir, mode))
            .args(["--set", timeout])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ready_end = std::time::Instant::now() + Duration::from_secs(10);
        while !std::fs::read_to_string(dir.path().join("logs/rivium-demo/rivium-demo.log"))
            .unwrap_or_default()
            .contains("services running")
        {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before ready"
            );
            if std::time::Instant::now() >= ready_end {
                let _ = child.kill();
                panic!("child never became ready");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for (index, signal) in signals.iter().enumerate() {
            if index > 0 {
                // Observe the first signal before sending the abort signal.
                let end = std::time::Instant::now() + Duration::from_secs(5);
                while !dir.path().join("stopping").exists() {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "child exited before stopping"
                    );
                    if std::time::Instant::now() >= end {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("child never observed the first signal");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            assert!(
                Command::new("kill")
                    .args(["-s", signal, &child.id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(exit) = child.try_wait().unwrap() {
                break exit;
            }
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                panic!("process did not stop");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit.code(), Some(expected));
    }

    for signal in ["TERM", "INT"] {
        let dir = TempDir::new().unwrap();
        let mut child = tests_integration::command()
            .args(args(&dir, "restart-backoff"))
            .args(["--set", "lifecycle.restart=in-process"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(15);
        // The fifth failure starts a 3.2s backoff, leaving ample signal delivery time.
        while std::fs::read_to_string(dir.path().join("logs/rivium-demo/rivium-demo.log"))
            .unwrap_or_default()
            .matches("service failed")
            .count()
            < 5
        {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before backoff"
            );
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child never reached restart backoff");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let builds = std::fs::read_to_string(dir.path().join("builds")).unwrap();
        assert_eq!(builds.lines().count(), 5);
        assert!(
            Command::new("kill")
                .args(["-s", signal, &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(exit) = child.try_wait().unwrap() {
                break exit;
            }
            if std::fs::read_to_string(dir.path().join("builds")).unwrap() != builds {
                let _ = child.kill();
                let _ = child.wait();
                panic!("a stop signal during restart backoff constructed another generation");
            }
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("process did not stop during restart backoff");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit.code(), Some(0));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("builds")).unwrap(),
            builds,
            "a stop signal during restart backoff must not construct another generation"
        );
    }
}

#[test]
fn blocking_timeout_refuses_restart_until_work_really_exits() {
    use std::sync::{Mutex, mpsc};

    type Work = (mpsc::Receiver<()>, mpsc::Sender<()>);
    static WORK: Mutex<Option<Work>> = Mutex::new(None);

    #[derive(Default, serde::Serialize, serde::Deserialize)]
    struct Config {}

    struct Blocking;

    impl rivium_host::App for Blocking {
        const NAME: &'static str = "blocking";
        const VERSION: &'static str = "test";
        type Config = Config;

        fn services(
            _: &Config,
            _: &rivium_host::Context<Config>,
        ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
            let work = WORK.lock().unwrap().take();
            Ok(vec![rivium::service(
                "worker",
                rivium::Kind::Frontline,
                move |ctx| async move {
                    if let Some((release, started)) = work {
                        ctx.spawn_blocking("operation", move || {
                            started.send(()).unwrap();
                            release.recv().unwrap();
                            Ok(())
                        })
                        .map_err(|error| rivium::Error::internal(error.to_string()))?;
                    }
                    ctx.ready();
                    ctx.stopped().await;
                    Ok(())
                },
            )])
        }
    }

    let (release, rx) = mpsc::channel();
    let (tx, started) = mpsc::channel();
    *WORK.lock().unwrap() = Some((rx, tx));
    let dir = TempDir::new().unwrap();
    let host = Host::new::<Blocking>();
    let args = vec!["--root".into(), dir.path().display().to_string()];

    assert_eq!(host.start(&args), Code::Ok);
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(host.stop(Duration::from_secs(1)), Code::StopTimedOut);
    assert_eq!(host.start(&args), Code::Busy);

    release.send(()).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let code = host.start(&args);
        if code == Code::Ok {
            break;
        }
        assert_eq!(code, Code::Busy);
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
}
