//! True process boundaries using only the public process test tools.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rivium_test::process::{self, Exited, Running};

fn subject() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("RIVIUM_VERIFY_PROCESS").expect("required ordinary subject"),
    );
    assert!(
        path.is_absolute() && path.is_file(),
        "required ordinary subject is not a file"
    );
    path
}

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rivium verify 測試 {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("unique test root");
        Self(path)
    }
    fn text(&self) -> &str {
        self.0.to_str().expect("UTF-8 root")
    }
    fn empty(&self) {
        assert_eq!(
            std::fs::read_dir(&self.0).unwrap().count(),
            0,
            "command must be side-effect free"
        );
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.0.join("logs/rivium-verify/rivium-verify.log"))
            .expect("flushed subject log")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove test root");
    }
}

fn once(args: &[&str]) -> Exited {
    let mut command = process::command(&subject());
    command.args(args);
    Running::start(command).wait()
}

fn status(exited: &Exited, code: i32) {
    assert_eq!(
        exited.code,
        Some(code),
        "exit contract: stdout={} stderr={}",
        exited.stdout,
        exited.stderr
    );
}

fn prefixed(exited: &Exited) {
    assert!(!exited.stderr.is_empty(), "expected fatal diagnosis");
    assert!(
        exited
            .stderr
            .lines()
            .all(|line| line.starts_with("rivium-verify: ")),
        "stderr prefix: {}",
        exited.stderr
    );
}

fn start(root: &Root, case: &str, extra: &[&str]) -> (Running, std::net::SocketAddr) {
    let case = format!("verify.case={case}");
    let mut args = vec!["--root", root.text(), "--set", &case];
    args.extend_from_slice(extra);
    let program = process::spawn(&subject(), &args);
    let address = program.addr_of("control");
    let identity = program.wait_for("verify identity", &[]);
    // A native runner must exec, not hide the subject behind another process's PID.
    if std::env::var_os("RIVIUM_VERIFY_NATIVE_RUNNER").is_some() {
        assert_eq!(
            identity["pid"].as_u64(),
            Some(u64::from(program.id())),
            "runner must exec the subject"
        );
    }
    if case != "verify.case=partly-ready" {
        program.wait_for("phase changed", &[("phase", "running")]);
    }
    (program, address)
}

fn control(address: std::net::SocketAddr, byte: u8) {
    use std::io::Write;
    let mut stream =
        std::net::TcpStream::connect_timeout(&address, Duration::from_secs(60)).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    stream.write_all(&[byte]).unwrap();
}

#[test]
fn host_boundaries() {
    let root = Root::new();
    for arg in ["--version", "-V"] {
        let version = once(&[arg]);
        status(&version, 0);
        assert_eq!(version.stdout, "rivium-verify 0.0.0\n", "version contract");
        assert!(version.stderr.is_empty());
    }
    for arg in ["--help", "-h"] {
        let help = once(&[arg]);
        status(&help, 0);
        assert!(help.stdout.contains("WorkDir=DIR") && help.stdout.contains("check-config"));
        assert!(help.stderr.is_empty());
    }
    let default = once(&["default-config"]);
    status(&default, 0);
    assert!(default.stdout.contains("[verify]") && default.stdout.contains("case = \"idle\""));
    let changed = once(&["default-config", "--set", "verify.case=hold-stop"]);
    status(&changed, 0);
    assert!(changed.stdout.contains("case = \"hold-stop\""));
    root.empty();

    for args in [
        vec!["--unknown"],
        vec!["oops"],
        vec!["--root"],
        vec!["--config"],
        vec!["--set"],
        vec!["--set", "bad"],
        vec!["--set", "=value"],
        vec!["WorkDir="],
        vec!["--root", root.text(), "WorkDir=other"],
        vec!["--config", "a", "--config", "b"],
        vec!["default-config", "--root", root.text()],
    ] {
        let exited = once(&args);
        status(&exited, 64);
        prefixed(&exited);
        assert!(
            exited.stdout.is_empty(),
            "usage errors must not start logging"
        );
    }
    root.empty();
    let missing = root.0.join("missing.toml");
    let exited = once(&["--root", root.text(), "--config", missing.to_str().unwrap()]);
    status(&exited, 78);
    prefixed(&exited);
    root.empty();
    let invalid = once(&[
        "--root",
        root.text(),
        "--set",
        "verify.case=idle",
        "--set",
        "lifecycle.stop_timeout=0s",
        "--set",
        "verify.unknown=x",
    ]);
    status(&invalid, 78);
    prefixed(&invalid);
    assert_eq!(
        invalid.stderr.lines().count(),
        2,
        "one diagnosis per configuration problem"
    );
    assert_eq!(
        invalid
            .stderr
            .matches("invalid configuration: lifecycle.stop_timeout")
            .count(),
        1
    );
    assert_eq!(
        invalid
            .stderr
            .matches("invalid configuration: verify.unknown")
            .count(),
        1
    );
    root.empty();
    let check = once(&[
        "check-config",
        "--root",
        root.text(),
        "--set",
        "verify.case=idle",
    ]);
    status(&check, 0);
    assert!(check.stdout.contains("verify.case") && check.stdout.contains("cli --set verify.case"));
    root.empty();

    // File < prefix environment < CLI, including relative paths from a different cwd.
    let cwd = Root::new();
    std::fs::write(cwd.0.join("input.toml"), "[verify]\ncase = 'hold-stop'\n").unwrap();
    let mut command = process::command(&subject());
    command
        .current_dir(&cwd.0)
        .args([
            "check-config",
            "--root",
            root.text(),
            "--config",
            "input.toml",
        ])
        .env("RIVIUM_VERIFY_VERIFY__CASE", "stop-restart");
    let file_env = Running::start(command).wait();
    status(&file_env, 0);
    assert!(
        file_env.stdout.contains("stop-restart")
            && file_env.stdout.contains("env RIVIUM_VERIFY_VERIFY__CASE")
    );
    let mut command = process::command(&subject());
    command
        .current_dir(&cwd.0)
        .args([
            "check-config",
            "--root",
            root.text(),
            "--config",
            "input.toml",
            "--set",
            "verify.case=idle",
        ])
        .env("RIVIUM_VERIFY_VERIFY__CASE", "stop-restart");
    let cli = Running::start(command).wait();
    status(&cli, 0);
    assert!(cli.stdout.contains("idle") && cli.stdout.contains("cli --set verify.case"));
    root.empty();
    let mut command = process::command(&subject());
    command
        .current_dir(&cwd.0)
        .args([
            "check-config",
            "--root",
            root.text(),
            "--config",
            "input.toml",
        ])
        .env("RIVIUM_VERIFY_VERIFY__CASE", "");
    let file = Running::start(command).wait();
    status(&file, 0);
    assert!(file.stdout.contains("hold-stop") && file.stdout.contains("file "));
    root.empty();

    for (case, detail) in [
        ("construct-error", "construction failed"),
        ("construct-panic", "construction panic"),
        ("empty", "no services"),
        ("startup-error", "startup failed"),
        ("startup-panic", "startup panic"),
    ] {
        let root = Root::new();
        let set = format!("verify.case={case}");
        let exited = once(&["--root", root.text(), "--set", &set]);
        status(&exited, 69);
        prefixed(&exited);
        assert_eq!(exited.stderr.lines().count(), 1, "one fatal host diagnosis");
        assert!(
            exited.stderr.contains(detail),
            "startup diagnosis: {}",
            exited.stderr
        );
        assert!(
            root.log().contains("stopped"),
            "exit flush includes stopped"
        );
    }
    // Directory permissions differ under elevated runners; a file-as-directory conflict does not.
    let conflict = Root::new();
    std::fs::write(conflict.0.join("blocker"), "not a directory").unwrap();
    let exited = once(&[
        "--root",
        conflict.text(),
        "--set",
        "verify.case=idle",
        "--set",
        "log.file.dir=blocker/child",
    ]);
    status(&exited, 73);
    prefixed(&exited);
    assert!(exited.stdout.is_empty());
    assert_eq!(std::fs::read_dir(&conflict.0).unwrap().count(), 1);

    for (case, byte, code, detail) in [
        ("idle", b'F', 0, ""),
        ("frontline-return", b'F', 70, "control"),
        ("idle", b'E', 70, "runtime failed"),
        ("idle", b'P', 70, "runtime panic"),
        ("idle", b'C', 70, "failed-child"),
        ("partly-ready", b'E', 70, "runtime failed"),
        ("idle", b'R', 75, ""),
    ] {
        let root = Root::new();
        let (program, address) = start(&root, case, &[]);
        control(address, byte);
        let exited = program.wait();
        status(&exited, code);
        if code == 70 {
            prefixed(&exited);
            assert_eq!(exited.stderr.lines().count(), 1);
            assert!(
                exited.stderr.contains(detail),
                "fault diagnosis: {}",
                exited.stderr
            );
            assert_eq!(
                exited
                    .events
                    .iter()
                    .filter(|event| event["message"] == "service failed")
                    .count(),
                1,
                "one primary error"
            );
        } else if code == 75 {
            prefixed(&exited);
            assert!(exited.stderr.contains("verification request"));
        } else {
            assert!(
                exited.stderr.is_empty(),
                "unexpected stderr: {}",
                exited.stderr
            );
        }
        assert_eq!(
            exited
                .events
                .iter()
                .filter(|event| event["message"] == "stopped")
                .count(),
            1
        );
        assert_eq!(
            root.log().matches(" stopped ").count(),
            1,
            "stopped flush exactly once"
        );
    }

    // No argv: copied ordinary bytes, executable-directory root, not cwd; no PATH fallback.
    let deployed = Root::new();
    let binary = deployed.0.join(if cfg!(windows) {
        "subject.exe"
    } else {
        "subject"
    });
    std::fs::copy(subject(), &binary).unwrap();
    std::fs::write(
        deployed.0.join("config.toml"),
        "[verify]\ncase = 'idle'\n[log.console]\nformat = 'json'\nfilter = 'info'\n",
    )
    .unwrap();
    let mut command = process::command(&binary);
    command
        .current_dir(&cwd.0)
        .env("RIVIUM_VERIFY_VERIFY__CASE", "idle");
    let program = Running::start(command);
    let address = program.addr_of("control");
    let identity = program.wait_for("verify identity", &[]);
    assert_eq!(
        Path::new(identity["root"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        deployed.0.canonicalize().unwrap()
    );
    assert_eq!(
        Path::new(identity["resolved"].as_str().unwrap())
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap(),
        deployed.0.canonicalize().unwrap()
    );
    control(address, b'F');
    status(&program.wait(), 0);
    assert!(
        !cwd.0.join("logs").exists(),
        "host must not change/use cwd as root"
    );

    // WorkDir is case insensitive and overrides the environment root.
    for option in [format!("wOrKdIr={}", root.text()), "--root".into()] {
        let mut command = process::command(&subject());
        command.args(["run", &option]);
        if option == "--root" {
            command.arg(root.text());
        }
        command
            .args([
                "--set",
                "verify.case=idle",
                "--set",
                "log.console.format=json",
                "--set",
                "log.console.filter=info",
            ])
            .env("RIVIUM_VERIFY_ROOT", cwd.text());
        let program = Running::start(command);
        let address = program.addr_of("control");
        let identity = program.wait_for("verify identity", &[]);
        assert_eq!(Path::new(identity["root"].as_str().unwrap()), root.0);
        control(address, b'F');
        status(&program.wait(), 0);
    }

    #[cfg(unix)]
    unix_signals();
}

#[cfg(unix)]
fn unix_signals() {
    for signal in ["TERM", "INT"] {
        let root = Root::new();
        let (program, _) = start(&root, "idle", &[]);
        program.signal("HUP");
        program.wait_for("reloading is not supported; ignored", &[]);
        program.signal(signal);
        let exited = program.wait();
        status(&exited, 0);
        assert!(exited.stderr.is_empty());
        let log = root.log();
        for phase in ["starting", "running", "stopping", "stopped"] {
            assert_eq!(
                log.matches(&format!("phase=\"{phase}\"")).count(),
                1,
                "phase once: {log}"
            );
        }
        assert!(log.contains("verify final"));
    }
    for (second, code) in [("INT", 130), ("TERM", 143)] {
        let root = Root::new();
        let (program, _) = start(&root, "hold-stop", &["--set", "lifecycle.stop_timeout=30s"]);
        program.signal("TERM");
        program.wait_for("verify stopping", &[]); // barrier still owns the service future
        program.signal(second);
        let exited = program.wait();
        status(&exited, code);
        prefixed(&exited);
        assert!(exited.stderr.contains("abandoned: control"));
    }
    let root = Root::new();
    let (program, _) = start(&root, "hold-stop", &["--set", "lifecycle.stop_timeout=1s"]);
    program.signal("TERM");
    program.wait_for("verify stopping", &[]);
    let exited = program.wait();
    status(&exited, 124);
    prefixed(&exited);
    assert!(exited.stderr.contains("abandoned: control"));

    for restart in ["exit", "in-process"] {
        let root = Root::new();
        let set = format!("lifecycle.restart={restart}");
        let (program, _) = start(&root, "stop-restart", &["--set", &set]);
        program.signal("TERM");
        program.wait_for("verify restart requested after stop", &[]);
        status(&program.wait(), 0); // external stop wins, even over a restart request
    }
    let root = Root::new();
    let (program, address) = start(&root, "idle", &["--set", "lifecycle.restart=in-process"]);
    control(address, b'R');
    let next = program.addr_of("control");
    program.wait_for("verify identity", &[]);
    program.wait_for("phase changed", &[("phase", "running")]);
    program.signal("TERM");
    status(&program.wait(), 0);
    assert_ne!(next.port(), 0);
}

#[test]
fn process_contract() {
    process::lifecycle_contract(&subject(), &["--set", "verify.case=idle"]);
}

#[test]
fn helper_environment() {
    assert_eq!(
        std::env::var("RUST_LOG").as_deref(),
        Ok("off"),
        "outer runner must set the hostile filter"
    );
    let root = Root::new();
    let (program, address) = start(&root, "idle", &[]);
    control(address, b'F');
    status(&program.wait(), 0);
    assert!(
        root.log().contains("verify final"),
        "helper must remove RUST_LOG before subject startup"
    );
}
