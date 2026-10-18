// Each integration target compiles this shared fixture independently.
#![allow(dead_code)]

use rivium_test::TempDir;
use rivium_util::log::{Category, Logger, Settings};
#[cfg(feature = "log-export")]
use std::time::{Duration, Instant};
use std::{
    path::{Path, PathBuf},
    process::Output,
    time::SystemTime,
};

pub(crate) fn log_directory(name: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("logs").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    (root, dir)
}

pub(crate) fn tiny_budget(name: &str) -> Settings {
    let mut settings = Settings::for_app(name);
    settings.file.max_file_size = 1024;
    settings.file.max_total_size = 4096;
    #[cfg(feature = "log-export")]
    {
        settings.export.max_size = 1024;
    }
    settings
}

pub(crate) fn category(name: impl Into<String>, filter: &str) -> Category {
    Category {
        name: name.into(),
        filter: filter.into(),
    }
}

pub(crate) fn default_logger(root: &TempDir, name: &str) -> rivium::Result<Logger> {
    Logger::new(name, root.path(), Settings::for_app(name), false, vec![])
}

pub(crate) fn run_child(test: &str, variable: &str, value: &str) -> Output {
    rivium_test::command(std::env::current_exe().unwrap())
        // Keep libtest's single-thread test-name prefix off the captured log line.
        .args(["--exact", test, "--nocapture", "--format=terse"])
        .env(variable, value)
        .output()
        .unwrap()
}

pub(crate) fn modified(path: impl AsRef<Path>, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

pub(crate) fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

#[cfg(feature = "log-export")]
pub(crate) fn request(from: &str, to: &str, sinks: &[&str]) -> rivium_util::log::ExportRequest {
    rivium_util::log::ExportRequest {
        from: from.into(),
        to: to.into(),
        sinks: sinks.iter().map(|sink| (*sink).into()).collect(),
    }
}

#[cfg(feature = "log-export")]
pub(crate) fn ready_archive(
    exporter: &rivium_util::log::Exporter,
    id: rivium_util::log::ExportId,
) -> rivium_util::log::Archive {
    ready_archive_with_timeout(exporter, id, Duration::from_secs(5))
}

#[cfg(feature = "log-export")]
pub(crate) fn ready_archive_with_timeout(
    exporter: &rivium_util::log::Exporter,
    id: rivium_util::log::ExportId,
    timeout: Duration,
) -> rivium_util::log::Archive {
    let until = Instant::now() + timeout;
    loop {
        match exporter.progress(id).unwrap() {
            rivium_util::log::ExportState::Running => {
                assert!(Instant::now() < until, "export timed out after {timeout:?}");
                std::thread::sleep(Duration::from_millis(10));
            }
            rivium_util::log::ExportState::Ready(archive) => return archive,
            state => panic!("export did not become ready: {state:?}"),
        }
    }
}
