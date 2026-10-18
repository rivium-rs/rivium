//! Public-API log throughput baseline; setup, shutdown and directory deletion are untimed.

use flate2::{Compression, write::GzEncoder};
use rivium_test::TempDir;
use rivium_util::log::{Logger, Settings};
use std::{
    fs,
    io::Write,
    time::{Duration, Instant},
};

const SAMPLES: usize = 7;
const EVENTS: usize = 500;

fn batch(archives: usize, rotate: bool, events: usize) -> f64 {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("logs/bench");
    fs::create_dir_all(&dir).unwrap();
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(b"valid pre-existing archive\n").unwrap();
    let archive = gzip.finish().unwrap();
    for seq in 1..=archives {
        fs::write(
            dir.join(format!("bench.2026-01-01.{seq:020}.log.gz")),
            &archive,
        )
        .unwrap();
    }

    let mut settings = Settings::for_app("bench");
    settings.console.filter = "off".into();
    settings.file.max_file_size = if rotate { 1024 } else { 1024 * 1024 };
    settings.file.max_total_size = if rotate { 4096 } else { 64 * 1024 * 1024 };
    settings.export.max_size = 1024;
    let budget = settings.file.max_total_size;
    let logger = Logger::new("bench", temp.path(), settings, false, vec![]).unwrap();
    let dispatch = logger.dispatch();
    let payload = "lifecycle workload with a representative log message ".repeat(5);

    let start = Instant::now();
    tracing::dispatcher::with_default(&dispatch, || {
        for seq in 0..events {
            tracing::info!(seq, "{payload}");
        }
    });
    // Include the barrier: enqueue speed alone does not measure the writer's work.
    logger.flush(Duration::from_secs(30)).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(logger.dropped(), 0);
    assert!(
        fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>()
            <= budget
    );

    logger.close(Duration::from_secs(30)).unwrap();
    elapsed.as_nanos() as f64 / events as f64
}

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--test");
    println!(
        "case, events_per_sample, samples, median_ns_per_event, min_ns_per_event, max_ns_per_event"
    );
    for (archives, rotate) in [(0, false), (1000, false), (0, true)] {
        let events = if smoke { 1 } else { EVENTS };
        batch(archives, rotate, events);
        if smoke {
            continue;
        }
        let mut samples: Vec<_> = (0..SAMPLES)
            .map(|_| batch(archives, rotate, events))
            .collect();
        samples.sort_by(f64::total_cmp);
        println!(
            "log/archives/{archives}/rotate/{rotate}, {EVENTS}, {SAMPLES}, {:.1}, {:.1}, {:.1}",
            samples[SAMPLES / 2],
            samples[0],
            samples[SAMPLES - 1]
        );
    }
}
