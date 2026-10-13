//! Process exit codes, diagnostics and read-only command-line operations.

use rivium_test::process::{command, spawn, Exited, Running};
use tests_integration::{test_binary, test_root, TestRoot};

#[cfg(unix)]
use std::time::{Duration, Instant};
#[cfg(unix)]
use tests_integration::send_signal;

#[test]
fn host_fault_build_exits_69() {
    let root = test_root();
    let exited = spawn(&test_binary("host_fault_build"), &root.args()).wait();
    assert_eq!(exited.code, Some(69));
    assert_eq!(
        exited.stderr,
        "fault-host: startup failed: the services cannot be built: build failure\n"
    );
    assert!(root
        .log("fault-host")
        .contains("stopped code=\"StartupFailed\" exit_code=69"));
}

#[test]
fn host_fault_ready_exits_69() {
    let root = test_root();
    let exited = spawn(&test_binary("host_fault_ready"), &root.args()).wait();
    assert_eq!(exited.code, Some(69));
    assert_eq!(
        exited.stderr,
        "fault-host: startup failed: service fault: ready failure\n"
    );
    assert!(root
        .log("fault-host")
        .contains("stopped code=\"StartupFailed\" exit_code=69"));
}

#[test]
fn host_fault_run_exits_70() {
    let root = test_root();
    let exited = spawn(&test_binary("host_fault_run"), &root.args()).wait();
    assert_eq!(exited.code, Some(70));
    assert_eq!(
        exited.stderr,
        "fault-host: stopped after a fault: service fault: run failure\n"
    );
    assert!(root
        .log("fault-host")
        .contains("stopped code=\"Fault\" exit_code=70"));
}

#[cfg(unix)]
#[test]
#[cfg_attr(qemu, ignore = "requires native Unix signal delivery")]
fn host_deadline_exits_124() {
    let root = test_root();
    let proc = spawn(&test_binary("host_deadline"), &root.args());
    proc.wait_for("phase changed", &[("phase", "running")]);
    let stopping = Instant::now();
    send_signal(proc.id(), libc::SIGTERM).expect("send SIGTERM");
    let exited = proc.wait();
    assert_eq!(exited.code, Some(124));
    assert!(stopping.elapsed() < Duration::from_secs(5));
    assert_eq!(
        exited.stderr,
        "deadline-host: stop timed out\ndeadline-host: abandoned: ignoring\n"
    );
    assert!(root
        .log("deadline-host")
        .contains("stopped code=\"StopTimedOut\" exit_code=124"));
}

#[test]
fn a_log_directory_that_cannot_be_created_exits_73() {
    let root = test_root();
    std::fs::write(root.path().join("blocked"), "a file, not a directory").unwrap();
    let mut args = root.args().to_vec();
    args.extend(["--set", "log.file.dir=blocked/logs"]);
    let exited = spawn(&test_binary("host"), &args).wait();
    assert_eq!(exited.code, Some(73));
    assert!(
        exited
            .stderr
            .starts_with("test-host: cannot write the log files:"),
        "{}",
        exited.stderr
    );
    assert_eq!(exited.stderr.lines().count(), 1);
    assert!(exited.events.is_empty(), "logging must not have started");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

// Unlike spawn(), this does not append logging flags: --help and --version stand alone.
fn cli(root: &TestRoot, args: &[&str]) -> Exited {
    let mut command = command(&test_binary("host_args"));
    command
        .args(args)
        .current_dir(root.path())
        .env("TEST_HOST_ROOT", root.path());
    Running::start(command).wait()
}

#[test]
fn version_and_help_exit_0_without_starting_or_writing_files() {
    let root = test_root();
    for flag in ["--version", "-V", "--help", "-h"] {
        let exited = cli(&root, &[flag]);
        assert_eq!((exited.code, exited.stderr.as_str()), (Some(0), ""));
        if matches!(flag, "--version" | "-V") {
            assert_eq!(exited.stdout, "test-host 0.0.0\n");
        } else {
            assert!(exited.stdout.contains("Usage:"), "{}", exited.stdout);
            assert!(exited.stdout.contains("WorkDir=DIR"), "{}", exited.stdout);
        }
        assert!(exited.events.is_empty());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }
}

#[test]
fn unknown_arguments_exit_64_without_starting() {
    let root = test_root();
    let exited = cli(&root, &["--bogus"]);
    assert_eq!(exited.code, Some(64));
    assert_eq!(
        exited.stderr,
        "test-host: unknown argument `--bogus`\ntest-host: usage: run `test-host --help`\n"
    );
    assert!(exited.stdout.is_empty());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn invalid_configuration_exits_78_without_starting() {
    let root = test_root();
    let exited = cli(&root, &["--set", "nope=1"]);
    assert_eq!(exited.code, Some(78));
    assert_eq!(
        exited.stderr,
        "test-host: invalid configuration: nope (cli --set nope): unknown key\n"
    );
    assert!(exited.stdout.is_empty());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn configuration_commands_are_read_only() {
    let root = test_root();
    let defaults = cli(&root, &["default-config"]);
    assert_eq!((defaults.code, defaults.stderr.as_str()), (Some(0), ""));
    assert!(defaults.stdout.contains("[lifecycle]"));
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());

    let file = root.path().join("config.toml");
    std::fs::write(&file, &defaults.stdout).unwrap();
    let checked = cli(
        &root,
        &[
            "check-config",
            "--config",
            "config.toml",
            "--set",
            "log.filter=debug",
        ],
    );
    assert_eq!((checked.code, checked.stderr.as_str()), (Some(0), ""));
    assert!(
        checked
            .stdout
            .lines()
            .any(|line| line == "log.filter = \"debug\" (cli --set log.filter)"),
        "{}",
        checked.stdout
    );
    assert!(checked.events.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), defaults.stdout);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}
