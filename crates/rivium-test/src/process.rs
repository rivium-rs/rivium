//! Process-level test tools: start a service program as a supervisor would, read the events it
//! logs, send it signals, and check the lifecycle contract that every program built on Rivium
//! keeps.
//!
//! Cross-compiled tests for a Unix target run under qemu-user, which does not run the target's
//! child processes. When cargo's target runner (`CARGO_TARGET_<TRIPLE>_RUNNER`) is set for such
//! a target, programs are started through it; the runner execs the emulator, so signals reach
//! the program directly.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// How long the tools wait for a program to log an event or to exit: programs under an
/// emulator are slow to start.
const WAIT: Duration = Duration::from_secs(60);

/// A command that runs `bin`: through cargo's target runner when the test's target is a Unix
/// one with a runner set, else directly.
#[must_use]
pub fn command(bin: &Path) -> Command {
    let triple = crate::TARGET.to_uppercase().replace(['-', '.'], "_");
    let runner = std::env::var(format!("CARGO_TARGET_{triple}_RUNNER")).ok();
    let mut parts: Vec<String> = (runner.filter(|_| cfg!(unix)))
        .map(|runner| runner.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    match parts.is_empty() {
        true => Command::new(bin),
        false => {
            let mut command = Command::new(parts.remove(0));
            command.args(parts).arg(bin);
            command
        }
    }
}

/// Starts the program `bin` with `args`, which log its events at `info` and above to standard
/// output as JSON for the tools to read: `--set log.console.filter=info --set
/// log.console.format=json` follow `args`.
///
/// ```no_run
/// # use std::path::Path;
/// let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/my-service");
/// let service = rivium_test::process::spawn(&bin, &["--set", "http.addr=127.0.0.1:0"]);
/// let addr = service.addr_of("http");
/// # let _ = addr;
/// # #[cfg(unix)]
/// service.signal("TERM");
/// assert_eq!(service.wait().code, Some(0));
/// ```
#[must_use]
pub fn spawn(bin: &Path, args: &[&str]) -> Running {
    let mut command = command(bin);
    let console = ["log.console.filter=info", "log.console.format=json"];
    command
        .args(args)
        .args(console.iter().flat_map(|set| ["--set", set]));
    Running::start(command)
}

/// A running program: its events and its standard error are read as they come. It is killed
/// if it is dropped before it has exited.
pub struct Running {
    child: Child,
    output: Arc<Output>,
    /// How many events earlier waits went through.
    seen: Cell<usize>,
}

/// What a program wrote, shared with the threads that read it.
#[derive(Default)]
struct Output {
    lines: Mutex<Lines>,
    changed: Condvar,
}

#[derive(Default)]
struct Lines {
    stdout: String,
    events: Vec<Map<String, Value>>,
    stderr: String,
    /// How many of the two streams have ended.
    ended: usize,
}

impl Running {
    /// Starts `command`, reading its standard output and error.
    ///
    /// # Panics
    ///
    /// When the program cannot be started.
    #[must_use]
    pub fn start(mut command: Command) -> Running {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {command:?}: {error}"));
        let output = Arc::new(Output::default());
        let stdout = child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn Read + Send>);
        let stderr = child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn Read + Send>);
        for (stream, is_stdout) in [(stdout, true), (stderr, false)] {
            let output = Arc::clone(&output);
            let stream = stream.expect("the streams are piped");
            std::thread::spawn(move || output.read(stream, is_stdout));
        }
        Running {
            child,
            output,
            seen: Cell::new(0),
        }
    }

    /// The process id.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Waits for the next event with this message and these field values, after the events
    /// that earlier waits went through, and returns it.
    ///
    /// # Panics
    ///
    /// When the program exits, or a minute passes, before the event arrives.
    pub fn wait_for(&self, message: &str, fields: &[(&str, &str)]) -> Map<String, Value> {
        let matches = |event: &Map<String, Value>| {
            let text = |key: &str| event.get(key).and_then(Value::as_str);
            text("message") == Some(message)
                && (fields.iter()).all(|(key, value)| text(key) == Some(*value))
        };
        let deadline = Instant::now() + WAIT;
        let mut lines = self.output.lines();
        loop {
            let next = lines.events.iter().enumerate().skip(self.seen.get());
            if let Some((index, event)) = next.into_iter().find(|(_, event)| matches(event)) {
                self.seen.set(index + 1);
                return event.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if lines.ended == 2 || left.is_zero() {
                let why = if lines.ended == 2 {
                    "the program exited"
                } else {
                    "a minute passed"
                };
                panic!(
                    "{why} without the event {message:?} {fields:?}\nstdout:\n{}\nstderr:\n{}",
                    lines.stdout, lines.stderr
                );
            }
            lines = (self.output.changed.wait_timeout(lines, left))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The address that the service named `service` listens on, from its next `listening`
    /// event.
    ///
    /// # Panics
    ///
    /// As [`wait_for`](Self::wait_for), or when the address is not a socket address.
    pub fn addr_of(&self, service: &str) -> SocketAddr {
        let event = self.wait_for("listening", &[("service.name", service)]);
        let addr = event.get("listen.addr").and_then(Value::as_str);
        addr.and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| panic!("not a socket address in {event:?}"))
    }

    /// Sends the signal named `name`, such as `TERM`, `INT`, `STOP` or `CONT`, with `kill`.
    ///
    /// # Panics
    ///
    /// When `kill` fails.
    #[cfg(unix)]
    pub fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .args(["-s", name, &self.id().to_string()])
            .status()
            .unwrap_or_else(|error| panic!("cannot run kill: {error}"));
        assert!(status.success(), "kill -s {name} failed: {status}");
    }

    /// Waits for the program to exit and returns how it ended.
    ///
    /// # Panics
    ///
    /// When the program has not exited after a minute; it is killed first.
    pub fn wait(mut self) -> Exited {
        let deadline = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the program's status") {
                break status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let lines = self.output.lines();
                panic!(
                    "the program did not exit within a minute\nstdout:\n{}\nstderr:\n{}",
                    lines.stdout, lines.stderr
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // The streams end once the program has exited, unless something it started holds them.
        let mut lines = self.output.lines();
        while lines.ended < 2 {
            let (next, timeout) = (self
                .output
                .changed
                .wait_timeout(lines, Duration::from_secs(5)))
            .unwrap_or_else(PoisonError::into_inner);
            lines = next;
            if timeout.timed_out() {
                break;
            }
        }
        Exited {
            code: status.code(),
            stdout: lines.stdout.clone(),
            events: lines.events.clone(),
            stderr: lines.stderr.clone(),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Output {
    fn read(&self, stream: Box<dyn Read + Send>, is_stdout: bool) {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let mut lines = self.lines();
            if is_stdout {
                let event = serde_json::from_str::<Map<String, Value>>(&line);
                lines.events.extend(event.ok());
                lines.stdout.push_str(&line);
                lines.stdout.push('\n');
            } else {
                lines.stderr.push_str(&line);
                lines.stderr.push('\n');
            }
            self.changed.notify_all();
        }
        self.lines().ended += 1;
        self.changed.notify_all();
    }

    fn lines(&self) -> MutexGuard<'_, Lines> {
        // Appending cannot leave the lines half written.
        self.lines.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How a program ended.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Exited {
    /// The exit code; `None` when a signal ended the process.
    pub code: Option<i32>,
    /// What it wrote to standard output.
    pub stdout: String,
    /// The JSON events among the lines of standard output.
    pub events: Vec<Map<String, Value>>,
    /// What it wrote to standard error.
    pub stderr: String,
}

/// Checks the lifecycle contract that every program built on Rivium keeps, on the program
/// `bin`, which must run with its defaults below an empty root directory:
///
/// - `--version` prints one line, `<name> <version>`, and exits with 0;
/// - an invalid configuration exits with 78 before anything starts, and says why on standard
///   error, each line after the name;
/// - `check-config` exits with 0 and writes no file;
/// - on Unix, SIGTERM stops the program with 0 within the default stop budget of 4 seconds, and
///   a second stop request cuts the stop short with 130 or 143.
///
/// ```no_run
/// #[test]
/// fn the_lifecycle_contract() {
///     rivium_test::process::lifecycle_contract(env!("CARGO_BIN_EXE_my-service").as_ref());
/// }
/// ```
///
/// # Panics
///
/// When the program breaks the contract.
pub fn lifecycle_contract(bin: &Path) {
    let root = Root::new();
    let run = |args: &[&str]| {
        let output = command(bin).args(args).output();
        output.unwrap_or_else(|error| panic!("cannot run {}: {error}", bin.display()))
    };

    let version = run(&["--version"]);
    let shown = String::from_utf8_lossy(&version.stdout);
    let words: Vec<&str> = shown.trim_end().split(' ').collect();
    let one_line = shown.lines().count() == 1;
    assert!(
        version.status.success() && one_line && words.len() == 2,
        "--version: {}, stdout {shown:?}",
        version.status
    );
    let name = words[0];

    let root_arg = root.0.to_str().expect("a UTF-8 temporary directory");
    let invalid = run(&["--root", root_arg, "--set", "lifecycle.stop_timeout=0s"]);
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert_eq!(
        invalid.status.code(),
        Some(78),
        "invalid configuration: {stderr}"
    );
    let prefix = format!("{name}: ");
    assert!(
        stderr.lines().count() > 0 && stderr.lines().all(|line| line.starts_with(&prefix)),
        "each line after the name: {stderr}"
    );
    assert!(
        stderr.contains("invalid configuration: lifecycle.stop_timeout"),
        "{stderr}"
    );
    assert_eq!(
        root.files(),
        Vec::<PathBuf>::new(),
        "nothing starts with an invalid configuration"
    );

    let check = run(&["check-config", "--root", root_arg]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        check.status.success(),
        "check-config: {}: {stderr}",
        check.status
    );
    assert_eq!(
        root.files(),
        Vec::<PathBuf>::new(),
        "check-config writes no file"
    );

    #[cfg(unix)]
    {
        let running = |args: &[&str]| {
            let program = spawn(bin, args);
            program.wait_for("phase changed", &[("phase", "running")]);
            program
        };
        let program = running(&["--root", root_arg]);
        let asked = Instant::now();
        program.signal("TERM");
        let exited = program.wait();
        let took = asked.elapsed();
        assert_eq!(exited.code, Some(0), "SIGTERM: {}", exited.stderr);
        assert!(took < Duration::from_secs(4), "SIGTERM took {took:?}");

        // Paused, so that both stop requests are waiting when it goes on.
        let program = running(&["--root", root_arg]);
        for signal in ["STOP", "TERM", "INT", "CONT"] {
            program.signal(signal);
        }
        let exited = program.wait();
        assert!(
            matches!(exited.code, Some(130 | 143)),
            "a second stop request: {:?}: {}",
            exited.code,
            exited.stderr
        );
    }
}

/// A new, empty directory below the system's temporary directory, removed when dropped.
struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rivium-contract-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create a temporary directory");
        Root(dir)
    }

    /// The files and directories in it.
    fn files(&self) -> Vec<PathBuf> {
        let entries = std::fs::read_dir(&self.0).expect("read the temporary directory");
        entries
            .map(|entry| entry.expect("an entry").path())
            .collect()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
