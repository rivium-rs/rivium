use rivium_test::TempDir;
#[cfg(unix)]
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(unix)]
struct Reap(Child);

#[cfg(unix)]
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn wait_ready(child: &mut Child, root: &std::path::Path) {
    let end = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(root.join("logs/rivium-demo/rivium-demo.log"))
        .unwrap_or_default()
        .contains("services running")
    {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fixture exited before ready"
        );
        assert!(Instant::now() < end, "fixture did not become ready");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(exit) = child.try_wait().unwrap() {
            return exit;
        }
        assert!(Instant::now() < end, "fixture did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_uncreatable_log_directory_exits_73_before_starting_services() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("blocked"), "not a directory").unwrap();
    let output = tests_integration::command()
        .arg("--root")
        .arg(root.path())
        .args(["--set", "log.file.dir=blocked/logs"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(73),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn cli_aliases_and_existing_config_inspection_are_read_only() {
    let root = TempDir::new().unwrap();
    for flag in ["--version", "-V", "--help", "-h"] {
        let output = tests_integration::command()
            .args([flag, "--root"])
            .arg(root.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);

    let defaults = tests_integration::command()
        .args(["default-config", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(defaults.status.success());
    assert!(String::from_utf8_lossy(&defaults.stdout).contains("[lifecycle]"));

    let file = root.path().join("config.toml");
    std::fs::write(&file, &defaults.stdout).unwrap();
    let checked = tests_integration::command()
        .args(["check-config", "--root"])
        .arg(root.path())
        .args([
            "--config",
            "config.toml",
            "--set",
            "log.console.filter=debug",
        ])
        .output()
        .unwrap();

    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert_eq!(std::fs::read(&file).unwrap(), defaults.stdout);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[cfg_attr(qemu, ignore = "native Unix signal delivery is verified separately")]
#[test]
fn normal_sigterm_flushes_the_final_run_record_before_process_exit() {
    let root = TempDir::new().unwrap();
    let mut child = Reap(
        tests_integration::command()
            .arg("--root")
            .arg(root.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(&mut child.0, root.path());
    assert!(
        Command::new("kill")
            .args(["-s", "TERM", &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(wait_exit(&mut child.0).code(), Some(0));
    let log =
        std::fs::read_to_string(root.path().join("logs/rivium-demo/rivium-demo.log")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("run finished"))
            .count(),
        1,
        "{log}"
    );
    assert!(log.contains("drain=Complete"), "{log}");
}

#[cfg(unix)]
#[cfg_attr(qemu, ignore = "native Unix signal delivery is verified separately")]
#[test]
fn sigquit_retains_default_signal_termination() {
    use std::os::unix::process::ExitStatusExt;

    let root = TempDir::new().unwrap();
    let original = tests_integration::command();
    // Only the child shell disables dumps; no unsafe pre_exec or process-global changes.
    let mut child = Reap(
        Command::new("sh")
            .args(["-c", "ulimit -c 0; exec \"$@\"", "rivium-signal-fixture"])
            .arg(original.get_program())
            .args(original.get_args())
            .arg("--root")
            .arg(root.path())
            .env_remove("RUST_LOG")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(&mut child.0, root.path());
    assert!(
        Command::new("kill")
            .args(["-s", "QUIT", &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let exit = wait_exit(&mut child.0);
    assert_eq!(exit.code(), None);
    assert_eq!(exit.signal(), Some(3));
    assert!(!root.path().join("core").exists());
}

#[test]
fn temporary_fixture_roots_are_isolated_and_removed() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let path = first.path().to_owned();
    assert_ne!(first.path(), second.path());
    std::fs::write(first.path().join("private"), "one fixture").unwrap();
    assert!(!second.path().join("private").exists());
    drop(first);
    assert!(!path.exists());
    assert!(second.path().exists());
}

#[test]
fn child_binary_is_located_without_runtime_cargo_binary_variables() {
    let command = tests_integration::command();
    assert!(
        command
            .get_program()
            .to_string_lossy()
            .ends_with("test-host")
            || command
                .get_args()
                .any(|arg| arg.to_string_lossy().ends_with("test-host"))
            || command
                .get_program()
                .to_string_lossy()
                .ends_with("test-host.exe")
    );
}

#[test]
fn usage_and_configuration_failures_do_not_create_runtime_files() {
    for (arguments, code) in [(vec!["--wrong"], 64), (vec!["--set", "unknown=1"], 78)] {
        let root = TempDir::new().unwrap();
        let output = tests_integration::command()
            .arg("--root")
            .arg(root.path())
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn process_reads_its_environment_root_and_explicit_root_wins() {
    let environment = TempDir::new().unwrap();
    let explicit = TempDir::new().unwrap();
    std::fs::write(environment.path().join("rivium-demo.toml"), "mode = 42\n").unwrap();
    let from_env = tests_integration::command()
        .env("RIVIUM_DEMO_ROOT", environment.path())
        .arg("check-config")
        .output()
        .unwrap();
    assert_eq!(from_env.status.code(), Some(78));
    let from_cli = tests_integration::command()
        .env("RIVIUM_DEMO_ROOT", environment.path())
        .args(["check-config", "--root"])
        .arg(explicit.path())
        .output()
        .unwrap();
    assert!(
        from_cli.status.success(),
        "{}",
        String::from_utf8_lossy(&from_cli.stderr)
    );
    assert_eq!(std::fs::read_dir(environment.path()).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(explicit.path()).unwrap().count(), 0);
}
