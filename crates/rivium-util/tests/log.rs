#![cfg(feature = "log")]

mod log_support;

use log_support::*;

use rivium_test::TempDir;
use rivium_util::log::{Logger, Settings};
use std::time::Duration;

#[test]
fn independent_log_filters_flush_and_reload() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("test");
    settings.filter = "info".into();
    settings.file.filter = "info,special=off".into();
    settings.files.push(category("special", "special=info"));
    let mut log = Logger::new("test", temp.path(), settings.clone(), false, vec![]).unwrap();
    tracing::dispatcher::with_default(&log.dispatch(), || {
        tracing::info!("ordinary");
        tracing::info!(target: "special", "category");
    });
    log.flush(Duration::from_secs(2)).unwrap();
    let main = std::fs::read_to_string(temp.path().join("logs/test/test.log")).unwrap();
    let cat = std::fs::read_to_string(temp.path().join("logs/test/special.log")).unwrap();
    assert!(main.contains("ordinary"));
    assert!(!main.contains("category"));
    assert!(cat.contains("category"));
    settings.file.filter = "error".into();
    assert!(log.reload(&settings).unwrap().is_empty());
    tracing::dispatcher::with_default(&log.dispatch(), || tracing::info!("filtered"));
    log.flush(Duration::from_secs(2)).unwrap();
    assert!(
        !std::fs::read_to_string(temp.path().join("logs/test/test.log"))
            .unwrap()
            .contains("filtered")
    );
}

#[test]
fn log_directory_has_one_owner_and_close_releases_it() {
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "single").unwrap();
    assert!(default_logger(&temp, "single").is_err());
    logger.close(Duration::from_secs(1)).unwrap();
    assert!(default_logger(&temp, "single").is_ok());
}

#[test]
fn oversized_events_are_dropped_whole_and_the_writer_recovers() {
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "bounded").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("{}", "中".repeat(30_000));
        tracing::info!("after oversized");
    });
    logger.close(Duration::from_secs(2)).unwrap();
    let output = std::fs::read_to_string(temp.path().join("logs/bounded/bounded.log")).unwrap();
    assert!(output.contains("after oversized"));
    assert!(!output.contains('中'));
    assert_eq!(logger.stats().oversized, 1);
    assert_eq!(logger.stats().queued_bytes, 0);
    assert_eq!(logger.stats().queued_events, 0);
}

#[test]
fn file_failure_is_sticky_but_close_releases_the_directory() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("failure");
    settings.file.max_file_size = 1024;
    let logger = Logger::new("failure", temp.path(), settings.clone(), false, vec![]).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("{}", "a".repeat(2048))
    });
    let first = logger.close(Duration::from_secs(2)).unwrap_err();
    let second = logger.close(Duration::ZERO).unwrap_err();
    assert_eq!(first.to_string(), second.to_string());
    assert_eq!(logger.pending_workers(), 0);
    assert!(logger.stats().write_failures > 0);
    Logger::new("failure", temp.path(), settings, false, vec![])
        .unwrap()
        .close(Duration::from_secs(2))
        .unwrap();
}

#[test]
fn categories_and_budget_are_validated_before_creating_files() {
    let mut settings = Settings::for_app("valid");
    settings.file.max_file_size = 1024;
    settings.file.max_total_size = u64::MAX;
    settings.files = (0..17).map(|i| category(format!("c{i}"), "info")).collect();
    assert!(settings.validate("valid").is_err());
    settings.files.truncate(16);
    assert!(settings.validate("valid").is_ok());
    settings.files[0].name = "../escape".into();
    assert!(settings.validate("valid").is_err());
    settings.files.clear();
    settings.file.max_file_size = u64::MAX;
    assert!(settings.validate("valid").is_err());
    settings.file.max_file_size = 1024;
    settings.file.filter = "[invalid".into();
    assert!(settings.validate("valid").is_err());
}

#[test]
fn span_updates_preserve_previous_context_fields() {
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "spans").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        let span = tracing::info_span!("run", run_id = 7, phase = tracing::field::Empty);
        let _entered = span.enter();
        span.record("phase", "stopping");
        tracing::info!("final event");
    });
    logger.close(Duration::from_secs(2)).unwrap();
    let data = std::fs::read_to_string(temp.path().join("logs/spans/spans.log")).unwrap();
    assert!(data.contains("run_id=7"), "{data}");
    assert!(data.contains("phase=stopping"), "{data}");
}

#[test]
fn console_json_preserves_structured_values_and_escapes_unicode() {
    const CHILD: &str = "RIVIUM_LOG_JSON_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let temp = TempDir::new().unwrap();
        let mut settings = Settings::for_app("json");
        settings.console.format = rivium_util::log::Format::Json;
        settings.console.filter = "info".into();
        let logger = Logger::new("json", temp.path(), settings, true, vec![]).unwrap();
        tracing::dispatcher::with_default(&logger.dispatch(), || {
            tracing::info!(answer = 42, valid = true, text = "中文\n\"quote\"", "hello");
        });
        logger.close(Duration::from_secs(2)).unwrap();
        return;
    }
    let output = run_child(
        "console_json_preserves_structured_values_and_escapes_unicode",
        CHILD,
        "1",
    );
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let line = stdout.lines().find(|line| line.starts_with('{')).unwrap();
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(value["answer"], 42);
    assert_eq!(value["valid"], true);
    assert_eq!(value["text"], "中文\n\"quote\"");
}

#[test]
fn human_sizes_and_expiry_round_trip_and_overflow_is_rejected() {
    let input = serde_json::json!({
        "file": {
            "max_file_size": "1 KiB",
            "max_total_size": "256 MiB"
        }
    });
    let settings: Settings = serde_json::from_value(input).unwrap();
    assert_eq!(settings.file.max_file_size, 1024);
    assert_eq!(settings.file.max_total_size, 268_435_456);
    assert_eq!(
        serde_json::from_value::<Settings>(serde_json::to_value(&settings).unwrap()).unwrap(),
        settings
    );
    assert!(
        serde_json::from_value::<Settings>(serde_json::json!({
            "file": {
                "max_file_size": "18446744073709551615 GiB"
            }
        }))
        .is_err()
    );
    #[cfg(feature = "log-export")]
    {
        let settings: Settings =
            serde_json::from_value(serde_json::json!({ "export": { "expire_after": "30m" } }))
                .unwrap();
        assert_eq!(settings.export.expire_after, Duration::from_secs(1800));
    }
}

#[test]
fn close_disables_retained_dispatchers_and_extra_layers() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tracing_subscriber::{Layer, Registry};

    struct Count(Arc<AtomicUsize>);

    impl Layer<Registry> for Count {
        fn on_event(
            &self,
            _: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, Registry>,
        ) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn emit() {
        tracing::info!("retained dispatch");
    }
    let count = Arc::new(AtomicUsize::new(0));
    let temp = TempDir::new().unwrap();
    let logger = Logger::new(
        "closed",
        temp.path(),
        Settings::for_app("closed"),
        false,
        vec![Box::new(Count(Arc::clone(&count)))],
    )
    .unwrap();
    let dispatch = logger.dispatch();
    tracing::dispatcher::with_default(&dispatch, emit);
    logger.close(Duration::from_secs(2)).unwrap();
    tracing::dispatcher::with_default(&dispatch, emit);
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn concurrent_producers_respect_queue_and_byte_bounds() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("pressure");
    settings.file.max_file_size = 1024;
    let logger = Logger::new("pressure", temp.path(), settings, false, vec![]).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let dispatch = logger.dispatch();
            scope.spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    for index in 0..4_000 {
                        tracing::info!(index, "{}", "queue pressure ".repeat(30));
                    }
                })
            });
        }
        for _ in 0..4_000 {
            let stats = logger.stats();
            assert!(stats.queued_events <= 1024);
            assert!(stats.queued_bytes <= 1024 * 64 * 1024);
            std::thread::yield_now();
        }
    });
    // A zero-budget close must still close admission and retain each live
    // worker's directory ownership until the retry observes completion.
    let _ = logger.close(Duration::ZERO);
    let replacement = default_logger(&temp, "pressure");
    if let Ok(replacement) = replacement {
        assert_eq!(logger.pending_workers(), 0);
        replacement.close(Duration::from_secs(2)).unwrap();
    }
    logger.close(Duration::from_secs(10)).unwrap();
    assert_eq!(logger.stats().queued_events, 0);
    assert_eq!(logger.stats().queued_bytes, 0);
    assert_eq!(logger.stats().write_failures, 0);
}

#[test]
fn panicking_extra_layer_construction_releases_all_directory_workers() {
    use tracing_subscriber::{Layer, Registry};

    struct Panics;

    impl Layer<Registry> for Panics {
        fn on_layer(&mut self, _: &mut Registry) {
            panic!("custom layer initialization");
        }
    }
    let temp = TempDir::new().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Logger::new(
            "layerpanic",
            temp.path(),
            Settings::for_app("layerpanic"),
            false,
            vec![Box::new(Panics)],
        )
    }));
    assert!(result.is_err());
    // Construction failure has no returned owner on which to retry cleanup;
    // all of its directory users must be gone before unwinding finishes.
    default_logger(&temp, "layerpanic")
        .unwrap()
        .close(Duration::from_secs(2))
        .unwrap();
}

#[test]
fn rotation_continues_recovered_sequence_and_rolls_previous_utc_day() {
    let (temp, dir) = log_directory("continued");
    const OLD_SEQUENCE: u64 = 9_000_000_000_000_000_000;
    std::fs::write(
        dir.join(format!("continued.2099-01-01.{OLD_SEQUENCE:020}.log")),
        "archive",
    )
    .unwrap();
    std::fs::write(dir.join("continued.log"), "previous day\n").unwrap();
    modified(dir.join("continued.log"), std::time::UNIX_EPOCH);
    let logger = default_logger(&temp, "continued").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("today"));
    logger.close(Duration::from_secs(2)).unwrap();
    let sequences: Vec<u64> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .split('.')
                .nth(2)
                .and_then(|n| n.parse().ok())
        })
        .collect();
    assert!(
        sequences.iter().any(|sequence| *sequence > OLD_SEQUENCE),
        "new rotations must continue the recovered sequence: {sequences:?}"
    );
    assert!(
        std::fs::read_to_string(dir.join("continued.log"))
            .unwrap()
            .contains("today")
    );
    assert!(std::fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .starts_with("continued.1970-01-01.")
    }));
}

#[test]
fn four_gib_file_budget_keeps_small_events_intact() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("wide");
    settings.file.max_file_size = 1u64 << 32;
    settings.file.max_total_size = 3u64 << 32;
    let logger = Logger::new("wide", temp.path(), settings, false, vec![]).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("small event under a large u64 budget")
    });
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(
        std::fs::read_to_string(temp.path().join("logs/wide/wide.log"))
            .unwrap()
            .contains("small event under a large u64 budget")
    );
}

#[test]
fn flush_reconciles_external_archives_and_accounting_failure_stops_growth() {
    let temp = TempDir::new().unwrap();
    let settings = tiny_budget("reconcile");
    let logger = Logger::new("reconcile", temp.path(), settings.clone(), false, vec![]).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("before"));
    logger.flush(Duration::from_secs(2)).unwrap();
    let dir = temp.path().join("logs/reconcile");
    for sequence in 1..=4 {
        std::fs::write(
            dir.join(format!("reconcile.2026-01-01.{sequence:020}.log")),
            [5; 1024],
        )
        .unwrap();
    }
    logger.flush(Duration::from_secs(2)).unwrap();
    assert!(
        !dir.join("reconcile.2026-01-01.00000000000000000001.log")
            .exists()
    );
    let blocker = dir.join("reconcile.2026-01-01.00000000000000000009.log");
    std::fs::create_dir(&blocker).unwrap();
    assert!(logger.flush(Duration::from_secs(2)).is_err());
    let before = std::fs::metadata(dir.join("reconcile.log")).unwrap().len();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("must not grow after failed accounting")
    });
    assert!(logger.close(Duration::from_secs(2)).is_err());
    assert_eq!(
        std::fs::metadata(dir.join("reconcile.log")).unwrap().len(),
        before
    );
    assert!(logger.stats().write_failures > 0);
    std::fs::remove_dir(blocker).unwrap();
    Logger::new("reconcile", temp.path(), settings, false, vec![])
        .unwrap()
        .close(Duration::from_secs(2))
        .unwrap();
}

#[test]
fn test_capture_observes_callsites_first_seen_on_another_thread() {
    const CHILD: &str = "RIVIUM_CAPTURE_CALLSITE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        fn emit() {
            tracing::warn!("same callsite after another thread");
        }
        let logs = rivium_test::Logs::default();
        let dispatch = logs.dispatch();
        tracing::dispatcher::with_default(&dispatch, || {
            std::thread::spawn(emit).join().unwrap();
            emit();
        });
        assert_eq!(
            logs.text()
                .matches("same callsite after another thread")
                .count(),
            1
        );
        return;
    }
    let result = run_child(
        "test_capture_observes_callsites_first_seen_on_another_thread",
        CHILD,
        "1",
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn console_colors_are_isolated_and_non_terminal_default_is_warn() {
    const CHILD: &str = "RIVIUM_COLOR_FILTER_CHILD";
    if let Some(mode) = std::env::var_os(CHILD) {
        use tracing_subscriber::Layer;
        let temp = TempDir::new().unwrap();
        let mut settings = Settings::for_app("output");
        let default_filter = mode == "default";
        if !default_filter {
            settings.console.filter = "info".into();
        }
        settings.console.color = if mode == "always" {
            rivium_util::log::Color::Always
        } else {
            rivium_util::log::Color::Never
        };
        let own = rivium_test::Logs::default();
        let extra = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(own.clone())
            .boxed();
        let mut logger =
            Logger::new("output", temp.path(), settings.clone(), true, vec![extra]).unwrap();
        tracing::dispatcher::with_default(&logger.dispatch(), || {
            let span = tracing::info_span!("unit", unit_id = 4);
            let _entered = span.enter();
            tracing::info!("FILE_INFO_SENTINEL");
            tracing::warn!("CONSOLE_WARN_SENTINEL");
        });
        settings.filter = "debug".into();
        logger.reload(&settings).unwrap();
        tracing::dispatcher::with_default(&logger.dispatch(), || {
            tracing::debug!("RELOADED_DEBUG_SENTINEL")
        });
        logger.close(Duration::from_secs(2)).unwrap();
        let file = std::fs::read_to_string(temp.path().join("logs/output/output.log")).unwrap();
        for text in [file.as_str(), own.text().as_str()] {
            assert!(!text.contains('\x1b'));
            assert!(text.contains("unit_id=4"));
            assert!(text.contains("FILE_INFO_SENTINEL"));
        }
        assert!(file.contains("RELOADED_DEBUG_SENTINEL"));
        return;
    }
    for mode in ["always", "never", "default"] {
        let result = run_child(
            "console_colors_are_isolated_and_non_terminal_default_is_warn",
            CHILD,
            mode,
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        assert_eq!(output.contains('\x1b'), mode == "always");
        assert!(output.contains("CONSOLE_WARN_SENTINEL"));
        assert_eq!(output.contains("FILE_INFO_SENTINEL"), mode != "default");
        assert!(!output.contains("RELOADED_DEBUG_SENTINEL"));
    }
}

#[test]
fn rotation_retains_raw_bytes_when_gzip_would_expand() {
    let (temp, dir) = log_directory("rawrotate");
    std::fs::write(dir.join("rawrotate.log"), b"x").unwrap();
    modified(dir.join("rawrotate.log"), std::time::UNIX_EPOCH);
    let logger = default_logger(&temp, "rawrotate").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("next day"));
    logger.close(Duration::from_secs(2)).unwrap();
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let raw = files
        .iter()
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rawrotate.1970-01-01.")
        })
        .unwrap();
    assert_eq!(raw.extension().unwrap(), "log");
    assert_eq!(std::fs::read(raw).unwrap(), b"x");
    assert!(
        !files
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "tmp"))
    );
}

#[test]
fn an_exhausted_recovered_sequence_is_rejected_without_destroying_history() {
    let (temp, dir) = log_directory("exhausted");
    let archive = dir.join(format!("exhausted.2026-01-01.{:020}.log", u64::MAX));
    std::fs::write(&archive, b"preserve history").unwrap();
    assert!(default_logger(&temp, "exhausted").is_err());
    assert_eq!(std::fs::read(archive).unwrap(), b"preserve history");
}

#[test]
fn logger_observes_a_callsite_first_reached_without_its_dispatch() {
    const CHILD: &str = "RIVIUM_LOGGER_CALLSITE_CHILD";
    if let Some(order) = std::env::var_os(CHILD) {
        fn emit() {
            tracing::warn!("SCOPED_LOGGER_SHARED_CALLSITE");
        }
        let temp = TempDir::new().unwrap();
        let logger = default_logger(&temp, "callsite").unwrap();
        let dispatch = logger.dispatch();
        if order == "scope-first" {
            tracing::dispatcher::with_default(&dispatch, || {
                std::thread::spawn(emit).join().unwrap();
                emit();
            });
        } else {
            std::thread::spawn(emit).join().unwrap();
            tracing::dispatcher::with_default(&dispatch, emit);
        }
        logger.close(Duration::from_secs(2)).unwrap();
        let text = std::fs::read_to_string(temp.path().join("logs/callsite/callsite.log")).unwrap();
        assert_eq!(text.matches("SCOPED_LOGGER_SHARED_CALLSITE").count(), 1);
        return;
    }
    for order in ["scope-first", "callsite-first"] {
        let result = run_child(
            "logger_observes_a_callsite_first_reached_without_its_dispatch",
            CHILD,
            order,
        );
        assert!(
            result.status.success(),
            "{order}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn compression_failure_is_reported_and_preserves_the_raw_input() {
    let (temp, dir) = log_directory("compressfail");
    std::fs::write(dir.join("compressfail.log"), b"keep").unwrap();
    modified(dir.join("compressfail.log"), std::time::UNIX_EPOCH);
    let logger = default_logger(&temp, "compressfail").unwrap();
    let prefix = "compressfail.1970-01-01.00000000000000000001.log";
    std::fs::create_dir(dir.join(format!("{prefix}.gz.tmp"))).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("rotate"));
    assert!(logger.close(Duration::from_secs(2)).is_err());
    assert_eq!(std::fs::read(dir.join(prefix)).unwrap(), b"keep");
    assert_eq!(
        std::fs::metadata(dir.join("compressfail.log"))
            .unwrap()
            .len(),
        0
    );
    assert!(logger.stats().write_failures > 0);
    assert_eq!(logger.pending_workers(), 0);
}

#[test]
fn startup_rejects_oversized_owned_files_without_deleting_unrelated_files() {
    let (temp, dir) = log_directory("oversized");
    let active = dir.join("oversized.log");
    let unknown = dir.join("oversized.unknown.log");
    std::fs::write(&active, [3; 3072]).unwrap();
    std::fs::write(&unknown, b"not a recognized owned archive").unwrap();
    let mut settings = Settings::for_app("oversized");
    settings.file.max_file_size = 1024;
    assert!(Logger::new("oversized", temp.path(), settings, false, vec![]).is_err());
    assert_eq!(std::fs::metadata(active).unwrap().len(), 3072);
    assert_eq!(
        std::fs::read(unknown).unwrap(),
        b"not a recognized owned archive"
    );
}

#[cfg(target_os = "android")]
#[test]
fn android_logcat_preserves_long_unicode_events_across_chunks() {
    let temp = TempDir::new().unwrap();
    let name = format!("rv-log-{}", std::process::id());
    let mut settings = Settings::for_app(&name);
    settings.file.filter = "off".into();
    settings.logcat.filter = "warn".into();
    let logger = Logger::new(&name, temp.path(), settings, false, vec![]).unwrap();
    let payload = "中é\0".repeat(1500);
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::warn!("LOGCAT_UNICODE_SENTINEL:{payload}")
    });
    logger.close(Duration::from_secs(2)).unwrap();
    let output = std::process::Command::new("/system/bin/logcat")
        .args(["-d", "-v", "raw", "-s", &format!("{name}:V"), "*:S"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let joined: String = text.lines().collect();
    assert!(joined.contains(&format!(
        "LOGCAT_UNICODE_SENTINEL:{}",
        payload.replace('\0', "\u{fffd}")
    )));
}

#[test]
fn concurrent_close_calls_confirm_the_same_accepted_queue() {
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "close").unwrap();
    let before = rivium_util::log::counters()[0];
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        for event in 0..100 {
            tracing::info!(event, "ACCEPTED_CLOSE_EVENT");
        }
    });
    let _ = logger.close(Duration::ZERO);
    std::thread::scope(|scope| {
        let waiters: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| logger.close(Duration::from_secs(2))))
            .collect();
        for waiter in waiters {
            waiter.join().unwrap().unwrap();
        }
    });
    logger.close(Duration::ZERO).unwrap();
    let file = std::fs::read_to_string(temp.path().join("logs/close/close.log")).unwrap();
    assert_eq!(file.matches("ACCEPTED_CLOSE_EVENT").count(), 100);
    assert_eq!(logger.dropped(), 0);
    assert_eq!(logger.pending_workers(), 0);
    assert!(rivium_util::log::counters()[0] >= before + file.len() as u64);
}

#[test]
fn structural_reload_is_reported_without_moving_live_outputs() {
    let temp = TempDir::new().unwrap();
    let original = Settings::for_app("cold");
    let mut logger = Logger::new("cold", temp.path(), original.clone(), false, vec![]).unwrap();
    let mut candidate = original.clone();
    candidate.filter = "debug".into();
    candidate.file.dir = "elsewhere".into();
    candidate.file.max_file_size *= 2;
    candidate.console.format = rivium_util::log::Format::Json;
    candidate.files.push(category("audit", "info"));
    #[cfg(feature = "log-export")]
    {
        candidate.export.max_size *= 2;
    }
    let cold = logger.reload(&candidate).unwrap();
    for key in [
        "log.file.dir",
        "log.file.budget",
        "log.console.format",
        "log.files",
    ] {
        assert!(cold.iter().any(|field| field == key));
    }
    #[cfg(feature = "log-export")]
    assert!(cold.iter().any(|field| field == "log.export"));
    assert_eq!(logger.settings().file.dir, original.file.dir);
    assert_eq!(
        logger.settings().file.max_file_size,
        original.file.max_file_size
    );
    assert_eq!(logger.settings().console, original.console);
    assert_eq!(logger.settings().files, original.files);
    #[cfg(feature = "log-export")]
    assert_eq!(logger.settings().export, original.export);
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::debug!("live hot filter"));
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(
        std::fs::read_to_string(temp.path().join("logs/cold/cold.log"))
            .unwrap()
            .contains("live hot filter")
    );
    assert!(!temp.path().join("elsewhere").exists());
}

#[test]
fn external_active_file_removal_is_a_visible_flush_failure() {
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "removed").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("before removal"));
    logger.flush(Duration::from_secs(2)).unwrap();
    let active = temp.path().join("logs/removed/removed.log");
    std::fs::remove_file(&active).unwrap();
    // Wine 10 keeps a delete-pending file visible while the writer owns it.
    // Ensure the missing-path precondition without weakening the error checks.
    if active.try_exists().unwrap() {
        std::fs::rename(&active, temp.path().join("removed.log")).unwrap();
    }
    assert!(!active.try_exists().unwrap());
    assert!(logger.flush(Duration::from_secs(2)).is_err());
    assert!(logger.close(Duration::from_secs(2)).is_err());
    assert_eq!(logger.pending_workers(), 0);
    assert!(logger.stats().write_failures > 0);
}
