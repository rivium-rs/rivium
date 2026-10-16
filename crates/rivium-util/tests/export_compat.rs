#![cfg(feature = "log-export")]
mod log_support;
use log_support::*;

use std::{
    fs::{self, File},
    io::Read,
    time::Duration,
};

use rivium_test::TempDir;

#[test]
fn exported_zip_round_trips_log_bytes() {
    let directory = TempDir::new().unwrap();
    let logger = default_logger(&directory, "compat").unwrap();
    tracing::dispatcher::with_default(&logger.dispatch(), || {
        tracing::info!("ZIP compatibility: 日志内容");
    });
    logger.flush(Duration::from_secs(5)).unwrap();
    let expected = fs::read(directory.path().join("logs/compat/compat.log")).unwrap();
    assert!(String::from_utf8_lossy(&expected).contains("日志内容"));

    let exporter = logger.exporter();
    let id = exporter
        .start(request("1970-01-01", "9999-12-31", &["compat"]))
        .unwrap();
    let archive = ready_archive(&exporter, id);

    let mut zip = zip::ZipArchive::new(File::open(&archive.path).unwrap()).unwrap();
    assert_eq!(zip.len(), 1);
    assert_eq!(archive.size, fs::metadata(&archive.path).unwrap().len());
    let mut entry = zip.by_name("compat.log").unwrap();
    assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
    let mut actual = Vec::new();
    entry.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, expected);
    drop(entry);
    drop(zip);

    exporter.cancel(id);
    assert!(!archive.path.exists());
}
