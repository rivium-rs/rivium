//! The outputs: filters, category files, console formats, installation and reloading, the
//! panic hook.

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Registry, fmt};

use super::install::{InstallError, LogInputs, build, filters, install};
use super::logcat::pieces;
use super::settings::{Category, Format, LogSettings};
use super::{flush, install_panic_hook};

/// Lines written to memory instead of standard output.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn writer(&self) -> BoxMakeWriter {
        let buffer = self.clone();
        BoxMakeWriter::new(move || buffer.clone())
    }
}

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rivium-output-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn inputs(dir: PathBuf, settings: LogSettings) -> LogInputs {
    LogInputs {
        name: "app".into(),
        settings,
        dir,
        console: true,
        logcat: false,
        layers: Vec::new(),
    }
}

#[test]
fn each_output_has_its_own_filter_and_each_category_its_own_file() {
    let dir = dir("filters");
    let mut settings = LogSettings::new("app");
    settings.console.filter = "warn".into();
    settings.files.push(Category {
        name: "proto".into(),
        filter: "proto=debug".into(),
    });
    let console = Buffer::default();
    let (files, layers, _) = build(&inputs(dir.clone(), settings), console.writer()).unwrap();
    tracing::subscriber::with_default(Registry::default().with(layers), || {
        tracing::info!(target: "app", "an info line");
        tracing::debug!(target: "proto", "a protocol frame");
        tracing::warn!(target: "app", "a warning");
        tracing::debug!(target: "app", "hidden everywhere");
    });
    assert!(files.flush(Duration::from_secs(10)));
    let main = std::fs::read_to_string(dir.join("app.log")).unwrap();
    let proto = std::fs::read_to_string(dir.join("proto.log")).unwrap();
    // Each line: a timestamp, then the level, the target and the message.
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(|line| line.split_once(' ').unwrap().1.trim_start().to_string())
            .collect()
    };
    assert_eq!(
        lines(&main),
        ["INFO app: an info line", "WARN app: a warning"]
    );
    assert_eq!(lines(&proto), ["DEBUG proto: a protocol frame"]);
    assert_eq!(lines(&console.text()), ["WARN app: a warning"]);
    files.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_console_can_write_json_with_the_fields_at_the_top() {
    let dir = dir("json");
    let mut settings = LogSettings::new("app");
    (settings.console.filter, settings.console.format) = ("info".into(), Format::Json);
    let console = Buffer::default();
    let (files, layers, _) = build(&inputs(dir.clone(), settings), console.writer()).unwrap();
    tracing::subscriber::with_default(Registry::default().with(layers), || {
        tracing::info!(
            service.name = "udp",
            listen.addr = "127.0.0.1:9000",
            "listening"
        );
    });
    let text = console.text();
    for part in [
        r#""message":"listening""#,
        r#""service.name":"udp""#,
        r#""listen.addr":"127.0.0.1:9000""#,
        r#""level":"INFO""#,
    ] {
        assert!(text.contains(part), "{text}");
    }
    files.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn outputs_without_a_filter_take_the_default_one() {
    let mut settings = LogSettings::new("app");
    settings.filter = "debug".into();
    settings.files.push(Category {
        name: "access".into(),
        filter: "access=info".into(),
    });
    let console = if io::stdout().is_terminal() {
        "debug"
    } else {
        "warn"
    };
    assert_eq!(
        filters(&settings),
        ["debug", "access=info", console, "warn"]
    );
    (settings.file.filter, settings.console.filter) = ("info".into(), "trace".into());
    assert_eq!(filters(&settings), ["info", "access=info", "trace", "warn"]);
}

/// The one test that installs logging for the whole test process.
#[test]
fn logging_is_installed_once_then_only_its_filters_change() {
    let dir = dir("install");
    let mut settings = LogSettings::new("app");
    settings.files.push(Category {
        name: "audit".into(),
        filter: "audit=info".into(),
    });
    // With the console on (at `warn` unless stdout is a terminal), the panics of failing tests
    // still reach the test output once logging is installed in this process.
    install(inputs(dir.clone(), settings.clone())).unwrap_or_else(|error| panic!("{error}"));
    tracing::debug!(target: "app", "not yet");
    log::debug!(target: "app", "not from the log crate either");

    settings.filter = "debug".into();
    assert!(install(inputs(dir.clone(), settings.clone())).is_ok());
    tracing::debug!(target: "app", "now at debug");
    log::debug!(target: "app", "and from the log crate");

    let moved = inputs(dir.join("elsewhere"), settings.clone());
    assert!(matches!(install(moved), Err(InstallError::Changed(keys)) if keys == ["log.file.dir"]));
    let mut more = settings.clone();
    more.files.push(Category {
        name: "extra".into(),
        filter: "info".into(),
    });
    assert!(
        matches!(install(inputs(dir.clone(), more)), Err(InstallError::Changed(keys)) if keys == ["log.files"])
    );
    // Sizes cannot change either, but keep their installed values with a warning.
    let mut resized = settings.clone();
    resized.file.max_file_size *= 2;
    assert!(install(inputs(dir.clone(), resized)).is_ok());

    assert!(flush(Duration::from_secs(10)).is_ok());
    let main = std::fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(
        !main.contains("not yet") && !main.contains("either"),
        "{main}"
    );
    assert!(
        main.contains("now at debug") && main.contains("and from the log crate"),
        "{main}"
    );
    assert!(
        main.contains("log settings changed; they take effect when the process restarts"),
        "{main}"
    );
    assert!(crate::stats().log_bytes >= main.len() as u64);
}

#[test]
fn the_panic_hook_logs_an_error_event_once() {
    install_panic_hook("app");
    install_panic_hook("app");
    let captured = Buffer::default();
    let subscriber =
        Registry::default().with(fmt::layer().with_ansi(false).with_writer(captured.writer()));
    tracing::subscriber::with_default(subscriber, || {
        let _ = std::panic::catch_unwind(|| panic!("boom"));
    });
    let text = captured.text();
    assert_eq!(text.matches("ERROR panic: panic").count(), 1, "{text}");
    let thread = std::thread::current();
    let expected = [
        "panic.message=\"boom\"".to_string(),
        // The path separator is the platform's.
        "panic.location=\"crates".to_string(),
        "tests.rs:".to_string(),
        format!("thread.name=\"{}\"", thread.name().unwrap()),
    ];
    for part in expected {
        assert!(text.contains(&part), "{part} in {text}");
    }
}

#[test]
fn long_logcat_text_is_split_at_character_boundaries() {
    fn split(text: &str, max: usize) -> Vec<&str> {
        pieces(text, max).collect()
    }
    assert_eq!(split("abcde", 2), ["ab", "cd", "e"]);
    assert_eq!(split("ééé", 3), ["é", "é", "é"]);
    assert_eq!(split("aé", 2), ["a", "é"]);
    assert_eq!(split("é", 1), ["é"]);
    assert!(split("", 4).is_empty());
    let long = "x".repeat(9_000);
    assert_eq!(
        split(&long, 4_000)
            .iter()
            .map(|piece| piece.len())
            .collect::<Vec<_>>(),
        [4_000, 4_000, 1_000]
    );
}

#[cfg(target_os = "android")]
#[test]
fn events_go_to_logcat_in_pieces() {
    let dir = dir("logcat");
    let mut inputs = inputs(dir.clone(), LogSettings::new("app"));
    (inputs.console, inputs.logcat) = (false, true);
    let (files, layers, handles) = build(&inputs, Buffer::default().writer()).unwrap();
    // The main file and logcat, which takes `warn` and above by default.
    assert_eq!(
        handles.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [0, 2]
    );
    tracing::subscriber::with_default(Registry::default().with(layers), || {
        tracing::warn!(target: "app", "a long warning: {}", "x".repeat(9_000));
    });
    files.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}
