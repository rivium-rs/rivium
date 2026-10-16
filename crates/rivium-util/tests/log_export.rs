#![cfg(feature = "log-export")]
mod log_support;
use log_support::*;

use rivium_test::TempDir;
use rivium_util::log::{Logger, Settings};
use std::time::Duration;

#[test]
fn rolling_and_export_keep_a_disk_budget() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("budget");
    settings.file.max_file_size = 1024;
    settings.file.max_total_size = 16384;
    settings.export.max_size = 4096;
    let log = Logger::new("budget", temp.path(), settings, false, vec![]).unwrap();
    for _ in 0..100 {
        tracing::dispatcher::with_default(&log.dispatch(), || {
            tracing::info!("{}", "a".repeat(400))
        });
    }
    log.flush(Duration::from_secs(5)).unwrap();
    let dir = temp.path().join("logs/budget");
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert!(
        entries
            .iter()
            .any(|e| e.file_name().to_string_lossy().ends_with(".gz"))
    );
    assert!(
        entries
            .iter()
            .map(|e| e.metadata().unwrap().len())
            .sum::<u64>()
            <= 16384
    );
    let export_root = TempDir::new().unwrap();
    let export_log = default_logger(&export_root, "budget").unwrap();
    tracing::dispatcher::with_default(&export_log.dispatch(), || tracing::info!("export marker"));
    export_log.flush(Duration::from_secs(2)).unwrap();
    let exporter = export_log.exporter();
    let id = exporter
        .start(request("2020-01-01", "2099-12-31", &["budget"]))
        .unwrap();
    let archive = ready_archive(&exporter, id);
    assert!(archive.path.exists());
    exporter.cancel(id);
    assert!(!archive.path.exists());
}

#[test]
fn exports_expire_without_a_progress_poll() {
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("expiry");
    settings.export.expire_after = Duration::from_secs(1);
    let logger = Logger::new("expiry", temp.path(), settings, false, vec![]).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("export"));
    logger.flush(Duration::from_secs(1)).unwrap();
    let exporter = logger.exporter();
    let id = exporter
        .start(request("2020-01-01", "2099-12-31", &[]))
        .unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(5);
    let archive = ready_archive(&exporter, id);
    while archive.path.exists() {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn closing_previous_logger_cannot_remove_the_next_loggers_archive() {
    use rivium_util::log::ExportState;

    let temp = TempDir::new().unwrap();
    let request = || request("1970-01-01", "9999-12-31", &[]);
    let old = default_logger(&temp, "generation").unwrap();
    let old_exporter = old.exporter();
    let old_id = old_exporter.start(request()).unwrap();
    let old_archive = ready_archive(&old_exporter, old_id);
    old.close(Duration::from_secs(2)).unwrap();
    assert!(!old_archive.path.exists());
    drop(old_exporter);
    drop(old);

    let new = default_logger(&temp, "generation").unwrap();
    let exporter = new.exporter();
    let id = exporter.start(request()).unwrap();
    let archive = ready_archive(&exporter, id);
    assert!(archive.path.exists());
    // Allow an old expiry worker to wake after the new generation is already ready.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        archive.path.exists(),
        "previous logger deleted the new archive"
    );
    assert!(matches!(
        exporter.progress(id).unwrap(),
        ExportState::Ready(_)
    ));
    new.close(Duration::from_secs(2)).unwrap();
}

#[test]
fn recovery_and_export_preserve_incompressible_raw_archives() {
    use std::{fs, io::Read};

    let (temp, dir) = log_directory("raw");
    let name = "raw.2026-01-01.00000000000000000001.log";
    let expected = noise(4096, 0x3456_789a_bcde_f012u64);
    fs::write(dir.join(name), &expected).unwrap();
    let mut settings = Settings::for_app("raw");
    settings.file.max_file_size = 4096;
    settings.file.max_total_size = 32768;
    settings.export.max_size = 16384;
    let logger = Logger::new("raw", temp.path(), settings, false, vec![]).unwrap();
    assert!(
        dir.join(name).exists(),
        "recovery must retain the complete raw input"
    );
    let exporter = logger.exporter();
    let id = exporter
        .start(request("1970-01-01", "9999-12-31", &["raw"]))
        .unwrap();
    let archive = ready_archive(&exporter, id);
    let mut zip = zip::ZipArchive::new(fs::File::open(archive.path).unwrap()).unwrap();
    let mut actual = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut actual).unwrap();
    assert_eq!(actual, expected);
    drop(zip);
    logger.close(Duration::from_secs(2)).unwrap();
}

#[test]
fn failed_export_initialization_does_not_leak_the_directory_lease() {
    let (temp, dir) = log_directory("init");
    std::fs::write(dir.join("exports"), "not a directory").unwrap();
    assert!(default_logger(&temp, "init").is_err());
    std::fs::remove_file(dir.join("exports")).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match default_logger(&temp, "init") {
            Ok(logger) => {
                logger.close(Duration::from_secs(1)).unwrap();
                break;
            }
            Err(error) => {
                assert!(std::time::Instant::now() < deadline, "{error}");
                std::thread::yield_now();
            }
        }
    }
}

#[test]
fn malformed_dates_and_parallel_jobs_are_rejected_without_losing_ready_archive() {
    use rivium_util::log::ExportState;
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "jobs").unwrap();
    let exporter = logger.exporter();
    let request = || request("2020-01-01", "2099-12-31", &[]);
    let mut invalid = request();
    invalid.from = "2026-02-30".into();
    assert!(exporter.start(invalid).is_err());
    let id = exporter.start(request()).unwrap();
    assert!(exporter.start(request()).is_err());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while matches!(exporter.progress(id).unwrap(), ExportState::Running) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let archive = exporter.archive(id).unwrap();
    assert!(exporter.start(request()).is_err());
    assert!(archive.path.exists());
    exporter.cancel(id);
    assert!(matches!(
        exporter.progress(id).unwrap(),
        ExportState::Cancelled
    ));
    assert!(!archive.path.exists());
    exporter.start(request()).unwrap();
    logger.close(Duration::from_secs(2)).unwrap();
    assert_eq!(logger.pending_workers(), 0);
}

#[test]
fn recovery_removes_only_owned_compression_temporary_files() {
    let (temp, dir) = log_directory("recover");
    let temporary = dir.join("recover.2026-01-01.00000000000000000001.log.gz.tmp");
    let unrelated = dir.join("external.tmp");
    std::fs::write(&temporary, "partial gzip").unwrap();
    std::fs::write(&unrelated, "user data").unwrap();
    let logger = default_logger(&temp, "recover").unwrap();
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(!temporary.exists());
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "user data");
}

#[test]
fn failed_export_removes_partial_zip_and_retains_a_bounded_result() {
    use rivium_util::log::ExportState;
    let temp = TempDir::new().unwrap();
    let mut settings = Settings::for_app("tinyzip");
    settings.export.max_size = 32;
    let logger = Logger::new("tinyzip", temp.path(), settings, false, vec![]).unwrap();
    let exporter = logger.exporter();
    let id = exporter
        .start(request("1970-01-01", "9999-12-31", &[]))
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match exporter.progress(id).unwrap() {
            ExportState::Running => {
                assert!(std::time::Instant::now() < until);
                std::thread::yield_now();
            }
            ExportState::Failed(_) => break,
            other => panic!("expected bounded export failure, got {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read_dir(temp.path().join("logs/tinyzip/exports"))
            .unwrap()
            .count(),
        0
    );
    logger.close(Duration::from_secs(2)).unwrap();
    assert_eq!(logger.pending_workers(), 0);
}

#[test]
fn cancellation_is_observed_before_close_confirms_export_worker_release() {
    use rivium_util::log::ExportState;
    let temp = TempDir::new().unwrap();
    let logger = default_logger(&temp, "cancel").unwrap();
    let exporter = logger.exporter();
    let id = exporter
        .start(request("1970-01-01", "9999-12-31", &[]))
        .unwrap();
    exporter.cancel(id);
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        exporter.progress(id).unwrap(),
        ExportState::Cancelled
    ));
    assert_eq!(logger.pending_workers(), 0);
    assert_eq!(
        std::fs::read_dir(temp.path().join("logs/cancel/exports"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn export_uses_utc_dates_and_filters_configured_sinks() {
    let (temp, dir) = log_directory("dates");
    for (name, content) in [
        ("dates.log", "active UTC leap day"),
        (
            "audit.2000-02-29.00000000000000000001.log",
            "selected category",
        ),
        ("dates.2000-02-28.00000000000000000002.log", "too early"),
        ("dates.2000-03-01.00000000000000000003.log", "too late"),
        ("outside.txt", "unowned"),
    ] {
        std::fs::write(dir.join(name), content).unwrap();
    }
    modified(
        dir.join("dates.log"),
        std::time::UNIX_EPOCH + Duration::from_secs(951_782_400),
    );
    let mut settings = Settings::for_app("dates");
    settings.files.push(category("audit", "off"));
    let logger = Logger::new("dates", temp.path(), settings, false, vec![]).unwrap();
    let exporter = logger.exporter();
    assert_eq!(exporter.sinks(), ["dates", "audit"]);
    for invalid in [
        "2023-02-29",
        "2026-02-30",
        "2026-13-01",
        "2026-00-10",
        "2026-10-00",
        "2026-1-01",
    ] {
        assert!(exporter.start(request(invalid, "9999-12-31", &[])).is_err());
    }
    for request in [
        request("2000-03-01", "2000-02-29", &[]),
        request("2000-02-29", "2000-03-01", &["unknown"]),
    ] {
        assert_eq!(
            exporter.start(request).unwrap_err().class(),
            rivium::error::Class::InvalidInput
        );
    }
    let id = exporter
        .start(request("2000-02-29", "2000-02-29", &[]))
        .unwrap();
    let archive = ready_archive(&exporter, id);
    let zip = zip::ZipArchive::new(std::fs::File::open(archive.path).unwrap()).unwrap();
    let names: std::collections::BTreeSet<_> = zip
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "dates.log".to_owned(),
            "audit.2000-02-29.00000000000000000001.log".to_owned()
        ])
    );
    drop(zip);
    exporter.cancel(id);
    let id = exporter
        .start(request("2000-02-29", "2000-02-29", &["audit"]))
        .unwrap();
    let archive = ready_archive(&exporter, id);
    let zip = zip::ZipArchive::new(std::fs::File::open(archive.path).unwrap()).unwrap();
    assert_eq!(zip.len(), 1);
    assert_eq!(
        zip.file_names().next().unwrap().unwrap().as_ref(),
        "audit.2000-02-29.00000000000000000001.log"
    );
    drop(zip);
    logger.close(Duration::from_secs(2)).unwrap();
}

#[test]
fn eviction_uses_sequence_across_categories_when_archive_dates_go_backwards() {
    let (temp, dir) = log_directory("sequence");
    let old = dir.join("audit.2099-01-01.00000000000000000001.log");
    let new = dir.join("sequence.1970-01-01.00000000000000000002.log");
    std::fs::write(&old, [1; 1024]).unwrap();
    std::fs::write(&new, [2; 1024]).unwrap();
    let mut settings = tiny_budget("sequence");
    settings.files.push(category("audit", "off"));
    let logger = Logger::new("sequence", temp.path(), settings, false, vec![]).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || tracing::info!("new active event"));
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(!old.exists(), "oldest sequence must be evicted first");
    assert!(
        new.exists(),
        "a lower calendar date does not make the newer sequence old"
    );
}

#[test]
fn startup_reserves_both_export_bytes_and_one_compression_workspace() {
    let (temp, dir) = log_directory("reserve");
    for sequence in 1..=6 {
        std::fs::write(
            dir.join(format!("reserve.2026-01-01.{sequence:020}.log")),
            [3; 1024],
        )
        .unwrap();
    }
    let mut settings = Settings::for_app("reserve");
    settings.file.max_file_size = 1024;
    settings.file.max_total_size = 8192;
    settings.export.max_size = 2048;
    let logger = Logger::new("reserve", temp.path(), settings, false, vec![]).unwrap();
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(
        !dir.join("reserve.2026-01-01.00000000000000000001.log")
            .exists()
    );
    for sequence in 2..=6 {
        assert!(
            dir.join(format!("reserve.2026-01-01.{sequence:020}.log"))
                .exists()
        );
    }
    let size: u64 = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| e.metadata().unwrap().len())
        .sum();
    assert_eq!(size, 5120);
}

#[test]
fn an_export_owner_cannot_be_replaced_while_packing_is_unfinished() {
    use rivium_util::log::ExportState;
    let (temp, dir) = log_directory("owner");
    let bytes = noise(4 * 1024 * 1024, 0x1234_5678_9abc_def0_u64);
    std::fs::write(dir.join("owner.2026-01-01.00000000000000000001.log"), bytes).unwrap();
    let logger = default_logger(&temp, "owner").unwrap();
    let retained_dispatch = logger.dispatch();
    let exporter = logger.exporter();
    let id = exporter
        .start(request("1970-01-01", "9999-12-31", &[]))
        .unwrap();
    let _ = logger.close(Duration::ZERO);
    drop(logger);
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let next = default_logger(&temp, "owner");
        if let Ok(next) = next {
            assert!(!matches!(
                exporter.progress(id).unwrap(),
                ExportState::Running
            ));
            next.close(Duration::from_secs(2)).unwrap();
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    }
    drop(retained_dispatch);
}

#[test]
fn active_files_before_the_epoch_export_on_their_utc_calendar_day() {
    let (temp, dir) = log_directory("past");
    std::fs::write(dir.join("past.log"), "last instant of 1969").unwrap();
    let logger = default_logger(&temp, "past").unwrap();
    let exporter = logger.exporter();
    for (seconds, day) in [
        (-1i64, "1969-12-31"),
        (0, "1970-01-01"),
        (951_782_400, "2000-02-29"),
        (1_704_067_199, "2023-12-31"),
        (1_704_067_200, "2024-01-01"),
        (1_791_460_800, "2026-10-08"),
        (1_791_504_000, "2026-10-09"),
        (4_107_542_400, "2100-03-01"),
    ] {
        let time = if seconds < 0 {
            std::time::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
        } else {
            std::time::UNIX_EPOCH + Duration::from_secs(seconds as u64)
        };
        modified(dir.join("past.log"), time);
        let id = exporter.start(request(day, day, &[])).unwrap();
        let archive = ready_archive(&exporter, id);
        let zip = zip::ZipArchive::new(std::fs::File::open(archive.path).unwrap()).unwrap();
        assert_eq!(zip.len(), 1, "{seconds} belongs to UTC day {day}");
        assert_eq!(
            zip.file_names().next().unwrap().unwrap().as_ref(),
            "past.log"
        );
        drop(zip);
        exporter.cancel(id);
    }
    logger.close(Duration::from_secs(2)).unwrap();
}

#[test]
fn eviction_failure_prevents_growth_in_every_category() {
    let (temp, dir) = log_directory("evictfail");
    let old = dir.join("evictfail.2026-01-01.00000000000000000001.log");
    std::fs::write(&old, [4; 1024]).unwrap();
    let other = dir.join("audit.2026-01-01.00000000000000000002.log");
    std::fs::write(&other, [5; 1024]).unwrap();
    let mut settings = tiny_budget("evictfail");
    settings.files.push(category("audit", "audit=info"));
    let logger = Logger::new("evictfail", temp.path(), settings, false, vec![]).unwrap();
    std::fs::remove_file(&old).unwrap();
    std::fs::create_dir(&old).unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("first write requires eviction");
        tracing::info!(target: "audit", "second category must not grow");
    });
    assert!(logger.close(Duration::from_secs(2)).is_err());
    assert_eq!(
        std::fs::metadata(dir.join("evictfail.log")).unwrap().len(),
        0
    );
    assert_eq!(std::fs::metadata(dir.join("audit.log")).unwrap().len(), 0);
    assert!(other.exists());
    assert!(logger.stats().write_failures > 0);
}

#[test]
fn category_rotation_holds_the_combined_budget_and_keeps_latest_events() {
    let temp = TempDir::new().unwrap();
    let mut settings = tiny_budget("categories");
    settings.file.filter = "main=info".into();
    settings.files.push(category("audit", "audit=info"));
    let logger = Logger::new("categories", temp.path(), settings, false, vec![]).unwrap();
    let before = rivium_util::log::counters()[2];
    let dir = temp.path().join("logs/categories");
    let mut seed = 0x1234_5678_u64;
    let payload: String = (0..850)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            char::from(b'!' + (seed % 90) as u8)
        })
        .collect();
    for index in 0..24 {
        tracing::dispatcher::with_default(&logger.dispatch(), || {
            if index % 2 == 0 {
                tracing::info!(target: "main", index, "{payload}");
            } else {
                tracing::info!(target: "audit", index, "{payload}");
            }
        });
        logger.flush(Duration::from_secs(2)).unwrap();
        let size: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().unwrap().is_file())
            .map(|entry| {
                let size = entry.metadata().unwrap().len();
                assert!(size <= 1024);
                size
            })
            .sum();
        assert!(
            size <= 2048,
            "log files consumed the compression/export reservations: {size}"
        );
    }
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(
        std::fs::read_to_string(dir.join("categories.log"))
            .unwrap()
            .contains("index=22")
    );
    assert!(
        std::fs::read_to_string(dir.join("audit.log"))
            .unwrap()
            .contains("index=23")
    );
    assert_eq!(logger.stats().write_failures, 0);
    assert!(rivium_util::log::counters()[2] > before);
}

#[test]
fn recovery_counts_both_raw_and_gzip_aliases_before_pruning() {
    use std::io::{Read, Write};
    let (temp, dir) = log_directory("aliases");
    let old = dir.join("aliases.2026-01-01.00000000000000000001.log");
    let raw = dir.join("aliases.2026-01-01.00000000000000000002.log");
    let gzip = raw.with_extension("log.gz");
    std::fs::write(&old, [1; 1024]).unwrap();
    std::fs::write(&raw, [2; 1024]).unwrap();
    let mut writer = flate2::write::GzEncoder::new(
        std::fs::File::create(&gzip).unwrap(),
        flate2::Compression::fast(),
    );
    writer.write_all(&[2; 1024]).unwrap();
    writer.finish().unwrap();
    let settings = tiny_budget("aliases");
    let logger = Logger::new("aliases", temp.path(), settings, false, vec![]).unwrap();
    logger.close(Duration::from_secs(2)).unwrap();
    assert!(
        !old.exists(),
        "raw and gzip aliases both consume real disk bytes"
    );
    assert_eq!(std::fs::read(raw).unwrap(), [2; 1024]);
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(gzip).unwrap())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, [2; 1024]);
}
