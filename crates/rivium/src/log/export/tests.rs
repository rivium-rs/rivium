//! Exports of a log directory, driven without installing logging: choosing files, packing them,
//! one export at a time, cancelling, expiring, and the archive's size limit.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rivium_error::{Class, kinds};

use super::{
    Date, ExportId, ExportRequest, ExportState, Exports, LogExporter, Progress, Task, Work,
};
use crate::log::settings::Export;

/// 2026-10-08T12:00:00Z.
const NOON: u64 = 1_791_460_800;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rivium-export-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn exporter(dir: &Path, max_size: u64, expire_after: Duration) -> (LogExporter, Arc<Exports>) {
    let settings = Export {
        max_size,
        expire_after,
    };
    let sinks = vec!["app".to_string(), "proto".to_string()];
    let exports = Arc::new(Exports::new("app", dir.to_path_buf(), sinks, &settings));
    (LogExporter(Some(Arc::clone(&exports))), exports)
}

fn day(text: &str) -> Date {
    text.parse().unwrap()
}

fn request(from: &str, to: &str, sinks: &[&str]) -> ExportRequest {
    ExportRequest {
        from: day(from),
        to: day(to),
        sinks: sinks.iter().map(ToString::to_string).collect(),
    }
}

/// Writes `text` to `name` in `dir`, last modified at `seconds` after the epoch.
fn file(dir: &Path, name: &str, text: &[u8], seconds: u64) {
    fs::write(dir.join(name), text).unwrap();
    let file = fs::File::options()
        .append(true)
        .open(dir.join(name))
        .unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

/// Bytes that deflate poorly: hexadecimal noise.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            b"0123456789abcdef"[(state & 15) as usize]
        })
        .collect()
}

/// Waits until the export is no longer running.
fn finished(exporter: &LogExporter, id: ExportId) -> Progress {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let progress = exporter.progress(id).unwrap();
        if progress.state != ExportState::Running || Instant::now() > deadline {
            return progress;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Each entry of the archive at `path`: its name, whether it is stored as it is, and its text.
fn entries(path: &Path) -> Vec<(String, bool, String)> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let stored = entry.compression() == zip::CompressionMethod::Stored;
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            (entry.name().to_string(), stored, text)
        })
        .collect()
}

/// The names in the directory that are export files.
fn exports_in(dir: &Path) -> Vec<String> {
    let names = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name());
    let names = names.map(|name| name.into_string().unwrap());
    names.filter(|name| name.starts_with("export-")).collect()
}

#[test]
fn days_are_utc_calendar_days_written_as_iso_dates() {
    assert_eq!(day("2026-10-08").to_string(), "2026-10-08");
    let noon = UNIX_EPOCH + Duration::from_secs(NOON);
    assert_eq!(Date::from(noon), day("2026-10-08"));
    assert_eq!(
        Date::from(noon + Duration::from_secs(12 * 3_600)),
        day("2026-10-09")
    );
    assert!(day("2026-09-30") < day("2026-10-01"));
    assert_eq!(day("2024-02-29").to_string(), "2024-02-29");
    assert_eq!(day("2000-03-01").to_string(), "2000-03-01");
    for bad in [
        "2023-02-29",
        "2026-02-30",
        "2026-13-01",
        "2026-00-10",
        "2026-10-00",
        "2026-1-01",
    ] {
        let error = bad.parse::<Date>().unwrap_err();
        assert_eq!(error.class(), Class::InvalidInput, "{bad}");
    }
    let read: Date = serde_json::from_str("\"2026-10-08\"").unwrap();
    assert_eq!(read, day("2026-10-08"));
    assert_eq!(serde_json::to_string(&read).unwrap(), "\"2026-10-08\"");
    let error = serde_json::from_str::<Date>("\"8 Oct 2026\"").unwrap_err();
    assert!(
        error.to_string().contains("not a day written like"),
        "{error}"
    );
}

#[test]
fn an_export_packs_the_chosen_days_and_log_files() {
    let dir = dir("chosen");
    let today = Date::today().to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    file(&dir, "app.log", b"line 1\nline 2\n", now);
    file(&dir, "app.2026-10-01.1.log.gz", b"compressed", NOON);
    file(&dir, "app.2026-10-02.2.log", b"rolled\n", NOON);
    file(&dir, "proto.2026-10-02.3.log", b"another log file\n", NOON);
    file(&dir, "app.2026-09-30.4.log.gz", b"too early", NOON);
    file(&dir, "proto.log", b"", now);
    file(&dir, "notes.txt", b"not a log file", NOON);
    let (exporter, _) = exporter(&dir, 1 << 20, Duration::from_secs(60));
    assert_eq!(exporter.sinks(), ["app", "proto"]);

    let id = exporter
        .start(request("2026-10-01", &today, &["app"]))
        .unwrap();
    // What is logged once the export started is not in it, even if the file rolls meanwhile.
    fs::rename(dir.join("app.log"), dir.join("app.2026-10-08.5.log")).unwrap();
    file(&dir, "app.log", b"after the start\n", now);
    let progress = finished(&exporter, id);
    assert_eq!((progress.percent, progress.state), (100, ExportState::Done));
    let archive = exporter.archive(id).unwrap();
    assert_eq!(archive.path, dir.join(format!("export-{id}.zip")));
    assert_eq!(archive.size, fs::metadata(&archive.path).unwrap().len());
    assert_eq!(
        archive.file_name,
        format!("app-logs-2026-10-01_{today}.zip")
    );
    assert_eq!(exporter.archive(id).unwrap(), archive, "as often as asked");
    let packed = entries(&archive.path);
    let names: Vec<(&str, bool)> = packed
        .iter()
        .map(|(name, stored, _)| (name.as_str(), *stored))
        .collect();
    assert_eq!(
        names,
        [
            ("app.log", false),
            ("app.2026-10-01.1.log.gz", true),
            ("app.2026-10-02.2.log", false),
            ("manifest.txt", false),
        ]
    );
    assert_eq!(packed[0].2, "line 1\nline 2\n");
    assert_eq!(packed[2].2, "rolled\n");
    let manifest = &packed[3].2;
    assert!(
        manifest.starts_with("Log export of app, made ")
            && manifest.contains("  app.2026-10-02.2.log\n")
            && !manifest.contains("Deleted"),
        "{manifest}"
    );

    // Every log file when none is named; nothing for days without logs.
    let id = exporter
        .start(request("2026-10-02", "2026-10-02", &[]))
        .unwrap();
    assert_eq!(finished(&exporter, id).state, ExportState::Done);
    let names: Vec<String> = entries(&exporter.archive(id).unwrap().path)
        .into_iter()
        .map(|entry| entry.0)
        .collect();
    assert_eq!(
        names,
        [
            "app.2026-10-02.2.log",
            "proto.2026-10-02.3.log",
            "manifest.txt"
        ]
    );
    let refused = [
        (request("2026-08-01", "2026-08-31", &[]), Class::NotFound),
        (
            request("2026-10-02", "2026-10-01", &[]),
            Class::InvalidInput,
        ),
        (
            request("2026-10-01", "2026-10-02", &["nope"]),
            Class::InvalidInput,
        ),
    ];
    for (request, class) in refused {
        let error = exporter.start(request.clone()).unwrap_err();
        assert_eq!(error.class(), class, "{request:?}: {error}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn packing_takes_what_was_there_at_the_start() {
    let dir = dir("deleted");
    for (name, text) in [
        ("app.log", "active\n"),
        ("app.2026-10-02.1.log", "deleted meanwhile\n"),
        ("app.2026-10-02.2.log", "compressed meanwhile\n"),
        ("app.2026-10-02.3.log", "still there\n"),
    ] {
        file(&dir, name, text.as_bytes(), NOON + 86_400);
    }
    let (_, exports) = exporter(&dir, 1 << 20, Duration::from_secs(60));
    let days = day("2026-10-02")..=day("2026-10-09");
    let selection = exports.select(days, &["app".to_string()]).unwrap();
    // The active file grows and rolls, a rolled file is deleted and another compressed.
    let mut active = fs::File::options()
        .append(true)
        .open(dir.join("app.log"))
        .unwrap();
    std::io::Write::write_all(&mut active, b"logged after the start\n").unwrap();
    drop(active);
    fs::rename(dir.join("app.log"), dir.join("app.2026-10-09.4.log")).unwrap();
    fs::remove_file(dir.join("app.2026-10-02.1.log")).unwrap();
    fs::rename(
        dir.join("app.2026-10-02.2.log"),
        dir.join("app.2026-10-02.2.log.gz"),
    )
    .unwrap();
    let part = dir.join("export-1.zip.part");
    let Ok((size, skipped)) = exports.pack(&part, selection, &Work::default()) else {
        panic!("packing failed");
    };
    assert_eq!((size, skipped), (fs::metadata(&part).unwrap().len(), 1));
    let packed = entries(&part);
    let names: Vec<(&str, bool)> = packed
        .iter()
        .map(|(name, stored, _)| (name.as_str(), *stored))
        .collect();
    assert_eq!(
        names,
        [
            ("app.log", false),
            ("app.2026-10-02.2.log.gz", true),
            ("app.2026-10-02.3.log", false),
            ("manifest.txt", false),
        ]
    );
    assert_eq!(packed[0].2, "active\n");
    let manifest = &packed[3].2;
    assert!(
        manifest.ends_with(
            "Deleted by the log directory's budget before they were packed:\n  app.2026-10-02.1.log\n"
        ),
        "{manifest}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_archive_over_its_limit_fails_and_leaves_no_file() {
    let dir = dir("limit");
    file(&dir, "app.2026-10-02.1.log.gz", &noise(64 * 1024), NOON);
    let (exporter, _) = exporter(&dir, 16 * 1024, Duration::from_secs(60));
    let id = exporter
        .start(request("2026-10-02", "2026-10-02", &[]))
        .unwrap();
    assert_eq!(finished(&exporter, id).state, ExportState::Failed);
    let error = exporter.archive(id).unwrap_err();
    assert_eq!(error.class(), Class::InvalidInput);
    assert_eq!(
        error.to_string(),
        "the archive would exceed log.export.max_size (16KiB): choose fewer days or log files"
    );
    assert_eq!(exports_in(&dir), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn one_export_runs_at_a_time_and_the_next_replaces_the_last() {
    let dir = dir("replace");
    file(&dir, "app.2026-10-02.1.log", b"rolled\n", NOON);
    let (exporter, exports) = exporter(&dir, 1 << 20, Duration::from_secs(60));
    let request = request("2026-10-02", "2026-10-02", &[]);
    let first = exporter.start(request.clone()).unwrap();
    assert_eq!(finished(&exporter, first).state, ExportState::Done);
    assert_eq!(exports_in(&dir), [format!("export-{first}.zip")]);

    // While an export runs, another is refused.
    let running = Task {
        id: 99,
        file_name: String::new(),
        state: ExportState::Running,
        work: Arc::default(),
        failure: None,
        size: 0,
    };
    let last = exports.slot().task.replace(running);
    let error = exporter.start(request.clone()).unwrap_err();
    assert_eq!(
        (error.etype(), error.to_string()),
        (
            kinds::CONFLICT,
            "log export 99 is still running".to_string()
        )
    );
    exports.slot().task = last;

    let second = exporter.start(request).unwrap();
    assert_eq!(finished(&exporter, second).state, ExportState::Done);
    assert_eq!(exports_in(&dir), [format!("export-{second}.zip")]);
    for gone in [
        exporter.progress(first).unwrap_err(),
        exporter.archive(first).unwrap_err(),
    ] {
        assert_eq!(gone.class(), Class::NotFound, "{gone}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cancelling_stops_an_export_or_removes_its_archive() {
    let dir = dir("cancel");
    file(&dir, "app.2026-10-02.1.log", &noise(16 << 20), NOON);
    let (exporter, exports) = exporter(&dir, 64 << 20, Duration::from_secs(60));
    let request = request("2026-10-02", "2026-10-02", &[]);
    let id = exporter.start(request.clone()).unwrap();
    let asked = Instant::now();
    // As the hosts do when they tear down: the wait is bounded, and an export that has not
    // ended by then ends on its own thread, so the state is read once it has.
    exports.cancel_running(Duration::from_millis(100));
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "{:?}",
        asked.elapsed()
    );
    let progress = finished(&exporter, id);
    assert_eq!(progress.state, ExportState::Cancelled, "{progress:?}");
    // Stopped while packing the 16 MiB, not cancelled as the archive was finished (99 %).
    assert!(progress.percent < 50, "{progress:?}");
    assert_eq!(exports_in(&dir), Vec::<String>::new());
    assert_eq!(exporter.archive(id).unwrap_err().class(), Class::NotFound);

    fs::write(dir.join("app.2026-10-02.1.log"), b"small\n").unwrap();
    let id = exporter.start(request.clone()).unwrap();
    assert_eq!(finished(&exporter, id).state, ExportState::Done);
    exporter.cancel(id);
    assert_eq!(exporter.progress(id).unwrap().state, ExportState::Cancelled);
    assert_eq!(exports_in(&dir), Vec::<String>::new());

    let id = exporter.start(request).unwrap();
    exporter.cancel(id);
    assert_eq!(finished(&exporter, id).state, ExportState::Cancelled);
    assert_eq!(exports_in(&dir), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_finished_archive_expires() {
    let dir = dir("expire");
    file(&dir, "app.2026-10-02.1.log", b"rolled\n", NOON);
    let (exporter, _) = exporter(&dir, 1 << 20, Duration::from_millis(200));
    let id = exporter
        .start(request("2026-10-02", "2026-10-02", &[]))
        .unwrap();
    assert_eq!(finished(&exporter, id).state, ExportState::Done);
    assert_eq!(exports_in(&dir).len(), 1);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(exporter.progress(id).unwrap_err().class(), Class::NotFound);
    assert_eq!(exports_in(&dir), Vec::<String>::new());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_logging_there_is_nothing_to_export() {
    let exporter = LogExporter(None);
    assert!(exporter.sinks().is_empty());
    let error = exporter
        .start(request("2026-10-02", "2026-10-02", &[]))
        .unwrap_err();
    assert_eq!(error.class(), Class::Unavailable);
    exporter.cancel(ExportId(1));
}
