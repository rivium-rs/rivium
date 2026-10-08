//! The program as a supervisor runs it: real signals, exit codes, standard error and the log
//! file (V-2, V-18). Not built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rivium_test::process::{Exited, Running, command, spawn};

const BIN: &str = env!("CARGO_BIN_EXE_udp-echo");
const FAULTS: &str = env!("CARGO_BIN_EXE_udp-echo-faults");

/// A new, empty root directory for one test.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("udp-echo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `bin` below `root`, with the echo socket on any free port, as the default has it.
fn start(bin: &str, root: &Path, args: &[&str]) -> Running {
    let mut all = vec!["--root", root.to_str().unwrap()];
    all.extend(args);
    spawn(Path::new(bin), &all)
}

/// Runs `bin` below `root` until it exits.
fn exit(bin: &str, root: &Path, args: &[&str]) -> Exited {
    start(bin, root, args).wait()
}

/// Sends a datagram and returns the answer.
fn echo(addr: SocketAddr, datagram: &[u8]) -> Vec<u8> {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket.send_to(datagram, addr).unwrap();
    let mut buffer = [0; 1_024];
    let (len, _) = socket.recv_from(&mut buffer).unwrap();
    buffer[..len].to_vec()
}

fn stderr(exited: &Exited) -> Vec<&str> {
    exited.stderr.lines().collect()
}

/// The main log file of the program below `root`.
fn log(root: &Path, name: &str) -> String {
    let file = root.join(format!("logs/{name}/{name}.log"));
    std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()))
}

#[cfg(unix)]
#[test]
fn sigterm_stops_the_service_with_0_and_each_step_is_logged_once() {
    let root = root("sigterm");
    let program = start(BIN, &root, &[]);
    let addr = program.addr_of("echo");
    assert_eq!(echo(addr, b"ping"), b"ping");
    program.wait_for("phase changed", &[("phase", "running")]);
    program.signal("TERM");
    let exited = program.wait();
    assert_eq!((exited.code, exited.stderr.as_str()), (Some(0), ""));
    let log = log(&root, "udp-echo");
    for (once, part) in [
        ("starting", "rivium::process: starting "),
        ("listening", "rivium::lifecycle::service: listening "),
        (
            "stopping",
            "phase changed phase=\"stopping\" reason=SIGTERM",
        ),
        ("outcome", "outcome outcome=\"stopped\""),
        (
            "stopped",
            "rivium::process: stopped code=\"Ok\" exit_code=0",
        ),
    ] {
        let found = log.lines().filter(|line| line.contains(part)).count();
        assert_eq!(found, 1, "{once} in\n{log}");
    }
    // The flush barrier: the last line before the exit is in the file.
    let last = log.lines().last().unwrap();
    assert!(
        last.contains("rivium::process: stopped code=\"Ok\" exit_code=0"),
        "{log}"
    );
}

#[cfg(unix)]
#[test]
fn a_log_export_in_progress_is_cancelled_before_the_exit() {
    let root = root("export");
    // A large rolled file from today, left by an earlier run: packing it takes a while.
    let dir = root.join("logs/udp-echo");
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let noise: Vec<u8> = (0..64 << 20)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            b"0123456789abcdef"[(state & 15) as usize]
        })
        .collect();
    let today = rivium::log::Date::today();
    std::fs::write(dir.join(format!("udp-echo.{today}.1.log")), noise).unwrap();
    let program = start(BIN, &root, &[]);
    let addr = program.addr_of("echo");
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(b"export", addr)
        .unwrap();
    program.wait_for("log export started", &[]);
    program.signal("TERM");
    let exited = program.wait();
    assert_eq!((exited.code, exited.stderr.as_str()), (Some(0), ""));
    let names = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name());
    let exports: Vec<_> = names
        .filter(|name| name.to_string_lossy().starts_with("export-"))
        .collect();
    assert_eq!(exports, Vec::<std::ffi::OsString>::new());
    let log = log(&root, "udp-echo");
    let cancelled = log
        .find("log export cancelled")
        .unwrap_or_else(|| panic!("{log}"));
    assert!(
        cancelled < log.find("rivium::process: stopped").unwrap(),
        "{log}"
    );
}

#[cfg(unix)]
#[test]
fn a_second_signal_cuts_the_stop_short_with_128_plus_its_number() {
    let root = root("second");
    // Blocking reads of 30s hold the stop up.
    let slow = [
        "--set",
        "echo.read_timeout=30s",
        "--set",
        "lifecycle.stop_timeout=60s",
    ];
    let program = start(BIN, &root, &slow);
    program.wait_for("phase changed", &[("phase", "running")]);
    program.signal("TERM");
    program.wait_for("phase changed", &[("phase", "stopping")]);
    program.signal("INT");
    let exited = program.wait();
    assert_eq!(exited.code, Some(130));
    assert_eq!(
        stderr(&exited),
        [
            "udp-echo: the stop was cut short by a second stop request (SIGINT)",
            // `stats` is a background service: asked to stop once `echo` has stopped. The list
            // follows the order of the services.
            "udp-echo: abandoned: echo/socket, stats"
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_service_still_running_at_the_deadline_is_abandoned_with_124() {
    let root = root("deadline");
    let slow = [
        "--set",
        "echo.read_timeout=30s",
        "--set",
        "lifecycle.stop_timeout=1s",
    ];
    let program = start(BIN, &root, &slow);
    program.wait_for("phase changed", &[("phase", "running")]);
    let asked = std::time::Instant::now();
    program.signal("TERM");
    let exited = program.wait();
    assert_eq!(exited.code, Some(124));
    // The stop timeout, then at most one more second for the host.
    assert!(
        asked.elapsed() < Duration::from_secs(2) + Duration::from_millis(500),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(
        stderr(&exited),
        [
            "udp-echo: stop timed out",
            "udp-echo: abandoned: echo/socket, stats"
        ]
    );
}

#[test]
fn a_service_that_cannot_start_fails_the_startup_with_69() {
    let root = root("startup");
    // A test address that no host has.
    let exited = exit(BIN, &root, &["--set", "echo.addr=192.0.2.1:9"]);
    assert_eq!(exited.code, Some(69));
    let lines = stderr(&exited);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("udp-echo: startup failed: service echo: binding 192.0.2.1:9: "),
        "{lines:?}"
    );
    let failed = log(&root, "udp-echo").matches("ERROR").count();
    assert_eq!(failed, 1, "the failure is logged once");
}

#[test]
fn a_panic_in_the_composition_root_fails_the_startup_with_69() {
    let root = root("panic");
    let exited = exit(FAULTS, &root, &["--set", "faults.panic_in_services=true"]);
    assert_eq!(exited.code, Some(69));
    assert_eq!(
        stderr(&exited),
        ["udp-echo-faults: startup failed: panic: a panic injected in the composition root"]
    );
}

#[test]
fn a_fault_of_a_ready_service_exits_with_70() {
    let root = root("fault");
    let exited = exit(FAULTS, &root, &["--set", "faults.fail_after_ms=200"]);
    assert_eq!(exited.code, Some(70));
    assert_eq!(
        stderr(&exited),
        [
            "udp-echo-faults: stopped after a fault: service fault, task timer: a fault injected in a service"
        ]
    );
}

#[test]
fn an_invalid_configuration_exits_with_78_and_one_line_per_problem() {
    let root = root("config");
    let exited = exit(
        BIN,
        &root,
        &["--set", "echo.addr=nowhere", "--set", "nope=1"],
    );
    assert_eq!(exited.code, Some(78));
    assert_eq!(
        stderr(&exited),
        [
            "udp-echo: invalid configuration: echo.addr (cli --set echo.addr): invalid socket address syntax",
            "udp-echo: invalid configuration: nope (cli --set nope): unknown key",
        ]
    );
    assert!(
        std::fs::read_dir(&root).unwrap().next().is_none(),
        "nothing started"
    );
}

#[test]
fn a_log_directory_that_cannot_be_created_exits_with_73() {
    let root = root("logdir");
    std::fs::write(root.join("blocked"), b"a file").unwrap();
    let exited = exit(BIN, &root, &["--set", "log.file.dir=blocked/logs"]);
    assert_eq!(exited.code, Some(73));
    let lines = stderr(&exited);
    assert!(
        lines.len() == 1 && lines[0].starts_with("udp-echo: cannot write the log files: "),
        "{lines:?}"
    );
}

#[test]
fn a_restart_request_exits_with_75() {
    let root = root("restart");
    let program = start(BIN, &root, &[]);
    let addr = program.addr_of("echo");
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.send_to(b"restart", addr).unwrap();
    let exited = program.wait();
    assert_eq!(exited.code, Some(75));
    let shown = format!(
        "udp-echo: restart requested: asked by {}",
        socket.local_addr().unwrap()
    );
    assert_eq!(stderr(&exited), [shown]);
}

#[test]
fn in_process_a_restart_request_builds_the_services_again() {
    let root = root("in-process");
    let program = start(BIN, &root, &["--set", "lifecycle.restart=in-process"]);
    let first = program.addr_of("echo");
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(b"restart", first)
        .unwrap();
    // Within 10s of the start: counted as a failure, so built again after a second.
    program.wait_for("restarting after a failure", &[]);
    let second = program.addr_of("echo");
    assert_eq!(echo(second, b"again"), b"again");
    #[cfg(unix)]
    {
        program.wait_for("phase changed", &[("phase", "running")]);
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
}

#[test]
fn without_arguments_the_root_is_the_executables_directory() {
    let root = root("no-args");
    let dir = root.join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join(Path::new(BIN).file_name().unwrap());
    std::fs::copy(BIN, &copy).unwrap();
    let mut command = command(&copy);
    // The installer starts the program without arguments, from another directory.
    command
        .current_dir(&root)
        .env("UDP_ECHO_LOG__CONSOLE__FILTER", "info")
        .env("UDP_ECHO_LOG__CONSOLE__FORMAT", "json");
    let program = Running::start(command);
    let starting = program.wait_for("starting", &[]);
    let shown = PathBuf::from(starting["root"].as_str().unwrap());
    assert_eq!(shown.canonicalize().unwrap(), dir.canonicalize().unwrap());
    program.addr_of("echo");
    assert!(dir.join("logs/udp-echo/udp-echo.log").is_file());
    #[cfg(unix)]
    {
        program.wait_for("phase changed", &[("phase", "running")]);
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
}

#[test]
fn work_dir_sets_the_root_as_the_installer_passes_it() {
    let root = root("workdir");
    let work_dir = format!("WorkDir={}", root.display());
    let program = spawn(Path::new(BIN), &[&work_dir]);
    let starting = program.wait_for("starting", &[]);
    assert_eq!(starting["root"].as_str(), root.to_str());
    program.addr_of("echo");
    assert!(root.join("logs/udp-echo/udp-echo.log").is_file());
}

#[test]
fn version_help_and_usage_errors() {
    let run = |args: &[&str]| command(Path::new(BIN)).args(args).output().unwrap();
    let version = run(&["--version"]);
    assert_eq!(
        (version.status.code(), version.stdout.as_slice()),
        (Some(0), &b"udp-echo 0.1.0\n"[..])
    );
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("WorkDir=DIR"));
    let usage = run(&["--bogus"]);
    assert_eq!(usage.status.code(), Some(64));
    assert_eq!(
        String::from_utf8_lossy(&usage.stderr),
        "udp-echo: unknown argument `--bogus`\nudp-echo: usage: run `udp-echo --help`\n"
    );
}

#[test]
fn check_config_shows_each_key_with_its_source_and_writes_nothing() {
    let root = root("check");
    let args = [
        "check-config",
        "--root",
        root.to_str().unwrap(),
        "--set",
        "echo.read_timeout=5s",
    ];
    let output = command(Path::new(BIN)).args(args).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in [
        "echo.read_timeout = \"5s\" (cli --set echo.read_timeout)",
        "stats.every = \"1m\" (default)",
        "lifecycle.restart = \"exit\" (default)",
    ] {
        assert!(
            stdout.lines().any(|shown| shown == line),
            "{line} in\n{stdout}"
        );
    }
    assert!(
        std::fs::read_dir(&root).unwrap().next().is_none(),
        "no file written"
    );
}
