//! P-SCALE task/config/log stress in one process, with externally sampled OS resources.
//! `--load`: 60s warmup + 3*600s load + 300s withdrawal, one independent process.
//! Run it three times. Separately, `--churn`: 1800s churn + 300s withdrawal.
//! Default/`--test` use one-second phases solely for harness validation.
//! Factors scale each tick's offered batch size, not a fixed open-loop arrival rate.
//! Queue/disk peaks are observed samples, not claims of continuously observed maxima.
//! Local filesystem writes/export do not simulate a slow disk or test HTTP/Host owners.

use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use rivium::{error::kinds, scope::TaskScope};
use rivium_test::TempDir;
use rivium_util::{
    config::{LoadLimits, Loader, Store},
    log::{ExportRequest, ExportState, Logger, Settings},
    task::{Context, DesiredError, Manager, Policy, ReconcileError},
};

const CAPACITY: usize = 1024;
const DOCUMENT: usize = 1024 * 1024;
const DISK: u64 = 64 * 1024 * 1024;

async fn worker(_: usize, _: [u8; 1024], context: Context) -> rivium::Result<()> {
    context.cancel.cancelled().await;
    Ok(())
}

fn disk_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return 0,
                Err(error) => panic!("cannot inspect log resources: {error}"),
            };
            if metadata.is_dir() {
                disk_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

struct Load {
    directory: TempDir,
    logger: Logger,
    manager: Manager<usize, [u8; 1024]>,
    scope: TaskScope,
    store: Store<BTreeMap<String, String>>,
    revision: u64,
}

impl Load {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let mut settings = Settings::for_app("scale");
        settings.console.filter = "off".into();
        settings.file.max_file_size = 1024 * 1024;
        settings.file.max_total_size = DISK;
        settings.export.max_size = 4 * 1024 * 1024;
        let logger = Logger::new("scale", directory.path(), settings, false, vec![]).unwrap();
        let scope = TaskScope::new();
        Self {
            directory,
            logger,
            manager: Manager::with_scope(Policy::default(), scope.clone()).unwrap(),
            scope,
            store: Store::new(BTreeMap::from([("label".into(), String::new())])),
            revision: 0,
        }
    }

    async fn tick(&mut self, factor: usize, churn: bool, tick: usize) -> [u128; 13] {
        let started = Instant::now();
        let count = if churn && !tick.is_multiple_of(2) {
            CAPACITY / 2
        } else {
            CAPACITY
        };
        let wanted = (0..count).map(|key| (key, [tick as u8; 1024])).collect();
        self.manager.reconcile(wanted, worker).await.unwrap();
        assert_eq!(self.manager.len(), count);
        assert_eq!(self.scope.active_tasks(), count);
        let rejected = if factor > 1 {
            let excessive = (0..CAPACITY * factor)
                .map(|key| (key, [42; 1024]))
                .collect();
            assert!(matches!(
                self.manager
                    .reconcile(excessive, |_, _, _| async {
                        panic!("over-capacity desired set must not enter a factory")
                    })
                    .await,
                Err(ReconcileError::InvalidDesired(DesiredError::TooMany {
                    requested,
                    capacity: CAPACITY,
                })) if requested == CAPACITY * factor
            ));
            assert_eq!(self.manager.len(), count);
            assert_eq!(self.scope.active_tasks(), count);
            1
        } else {
            0
        };
        let task_batch_ns = started.elapsed().as_nanos();

        let mut text = format!("label='{}'\n", "x".repeat(DOCUMENT - 9));
        assert_eq!(text.len(), DOCUMENT);
        let defaults = BTreeMap::from([("label".to_owned(), String::new())]);
        let document = Loader::new(&defaults)
            .unwrap()
            .limits(LoadLimits::default())
            .load_text(Some(&text))
            .unwrap();
        let decoded: BTreeMap<String, String> = document.decode().unwrap();
        self.revision += 1;
        assert_eq!(self.store.publish(decoded).unwrap(), self.revision);
        let handle = self.store.handle();
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.generation, self.revision);
        assert_eq!(snapshot.value["label"].len(), DOCUMENT - 9);
        text.push_str(&" ".repeat(if factor == 1 {
            1
        } else {
            DOCUMENT * (factor - 1)
        }));
        assert_eq!(
            Loader::new(&defaults)
                .unwrap()
                .load_text(Some(&text))
                .unwrap_err()
                .kind(),
            kinds::PAYLOAD_TOO_LARGE
        );
        assert_eq!(handle.generation(), self.revision);

        let exporter = self.logger.exporter();
        let export = exporter
            .start(ExportRequest {
                from: "1970-01-01".into(),
                to: "9999-12-31".into(),
                sinks: vec![],
            })
            .unwrap();
        let queue_peak = Arc::new(AtomicUsize::new(0));
        let bytes_peak = Arc::new(AtomicUsize::new(0));
        let before = self.logger.stats();
        let producers = if factor == 1 { 1 } else { 8 };
        let events = 1024 * factor / producers;
        let size = [128, 4096, 64 * 1024 - 512, 64 * 1024 + 1][tick % 4];
        let dispatch = self.logger.dispatch();
        let logger = &self.logger;
        let enqueue_started = Instant::now();
        std::thread::scope(|threads| {
            for _ in 0..producers {
                let dispatch = dispatch.clone();
                let queue_peak = queue_peak.clone();
                let bytes_peak = bytes_peak.clone();
                threads.spawn(move || {
                    let payload = "x".repeat(size);
                    tracing::dispatcher::with_default(&dispatch, || {
                        for _ in 0..events {
                            tracing::info!("{payload}");
                            let stats = logger.stats();
                            assert!(stats.queued_events <= 1024);
                            assert!(stats.queued_bytes <= 1024 * 64 * 1024);
                            queue_peak.fetch_max(stats.queued_events, Ordering::Relaxed);
                            bytes_peak.fetch_max(stats.queued_bytes, Ordering::Relaxed);
                        }
                    });
                });
            }
        });
        let enqueue_ns = enqueue_started.elapsed().as_nanos();
        self.logger.flush(Duration::from_secs(30)).unwrap();
        let export_deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match exporter.progress(export).unwrap() {
                ExportState::Ready(_) => break,
                ExportState::Running => {
                    assert!(Instant::now() < export_deadline, "export did not finish");
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                other => panic!("export failed: {other:?}"),
            }
        }
        let disk = disk_bytes(self.directory.path());
        assert!(disk <= DISK);
        exporter.cancel(export);
        let after = self.logger.stats();
        assert_eq!(after.write_failures, 0);
        assert_eq!(after.queued_events, 0);
        assert_eq!(after.queued_bytes, 0);
        if size > 64 * 1024 {
            assert_eq!(
                after.oversized - before.oversized,
                (producers * events) as u64
            );
        } else {
            assert_eq!(after.oversized - before.oversized, 0);
        }
        assert!(
            after.dropped_full - before.dropped_full + after.oversized - before.oversized
                <= (producers * events) as u64
        );
        [
            count as u128,
            rejected,
            task_batch_ns,
            enqueue_ns,
            queue_peak.load(Ordering::Relaxed) as u128,
            bytes_peak.load(Ordering::Relaxed) as u128,
            (after.dropped_full - before.dropped_full) as u128,
            (after.oversized - before.oversized) as u128,
            disk as u128,
            self.revision as u128,
            (producers * events) as u128,
            size as u128,
            text.len() as u128,
        ]
    }

    async fn close(&mut self) {
        self.manager.shutdown().await.unwrap();
        assert!(self.manager.is_empty());
        assert_eq!(self.scope.active_tasks(), 0);
        self.logger.close(Duration::from_secs(30)).unwrap();
        assert_eq!(self.logger.pending_workers(), 0);
    }
}

fn main() {
    let load_mode = std::env::args().any(|arg| arg == "--load");
    let churn_mode = std::env::args().any(|arg| arg == "--churn");
    assert!(
        !(load_mode && churn_mode),
        "select --load or --churn separately"
    );
    let smoke = (!load_mode && !churn_mode) || std::env::args().any(|arg| arg == "--test");
    if smoke {
        eprintln!(
            "resources_scale mode=smoke planned_phase_seconds=17; not P-SCALE acceptance evidence"
        );
    } else if load_mode {
        eprintln!(
            "resources_scale mode=load planned_phase_seconds=2160; run three independent processes with external sampler/watchdog"
        );
    } else {
        eprintln!(
            "resources_scale mode=churn planned_phase_seconds=2100; external sampler/watchdog required"
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(
        output,
        "pid,elapsed_ms,round,phase,factor,tick,active,rejected,task_batch_ns,enqueue_batch_ns,queue_observed_peak,queue_bytes_observed_peak,dropped_full,oversized,disk_observed_bytes,revision,offered_events,payload_bytes,rejected_config_bytes"
    )
    .unwrap();
    runtime.block_on(async {
        let mut load: Option<Load> = None;
        let mut tick = 0;
        let mut phases = Vec::new();
        for round in 0..if smoke {
            3
        } else if load_mode {
            1
        } else {
            0
        } {
            phases.push((round, "warmup", 1, 60));
            for factor in [1, 2, 10] {
                phases.push((round, "load", factor, 600));
            }
            phases.push((round, "withdraw", 0, 300));
        }
        if smoke || churn_mode {
            phases.push((3, "churn", 1, 1800));
            phases.push((3, "withdraw", 0, 300));
        }
        for (round, phase, factor, seconds) in phases {
            if factor == 0 {
                if let Some(mut previous) = load.take() {
                    let directory = previous.directory.path().to_owned();
                    Load::close(&mut previous).await;
                    drop(previous);
                    assert!(
                        !directory.exists(),
                        "withdrawal did not remove its owned directory"
                    );
                }
            } else if load.is_none() {
                load = Some(Load::new());
            }
            let deadline = Instant::now() + Duration::from_secs(if smoke { 1 } else { seconds });
            loop {
                let sample_start = Instant::now();
                let values = if let Some(current) = &mut load {
                    current.tick(factor, phase == "churn", tick).await
                } else {
                    [0; 13]
                };
                write!(
                    output,
                    "{},{},{round},{phase},{factor},{tick}",
                    std::process::id(),
                    started.elapsed().as_millis()
                )
                .unwrap();
                for value in values {
                    write!(output, ",{value}").unwrap();
                }
                writeln!(output).unwrap();
                output.flush().unwrap();
                tick += 1;
                if Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1).saturating_sub(sample_start.elapsed()))
                    .await;
            }
        }
    });
}
