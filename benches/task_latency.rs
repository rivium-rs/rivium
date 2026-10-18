//! Single-reconcile replacement latency and supplemental mixed recovery episodes.
//!
//! Copy this file unchanged into historical aa48a03 and use the same bench entry and
//! Tokio features. Passing a Vec explicitly adapts its IntoIterator API without
//! moving input allocation outside the measured boundary. No production shim is used.
//! The 1 KiB spec is inline bytes; changed keys rotate deterministically. The changed
//! count is ceil(N * percent / 100). Mixed cases divide affected keys among successful
//! exit, failed exit and spec replacement, rotating the assignment every round.
//! Mixed exit signals and scope-count drain are fixture setup before timing. Scope
//! accounting can precede JoinHandle completion; a 1 ns common backoff can also require
//! another observation. Mixed recovery episodes therefore include every reconcile,
//! desired Vec construction and intervening yield until all affected tasks restart.
//! Their percentiles are episode latency, not single-reconcile P-TASK gate samples.
//! Replacement samples always contain exactly one reconcile. Both measurements begin
//! before desired Vec construction and end on the final reconcile return. These are
//! closed-loop samples, not an open-loop production arrival distribution.

use std::{
    hint::black_box,
    io::{BufWriter, Write},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use rivium::scope::TaskScope;
use rivium_util::task::{Context, Manager, Policy};
use tokio::{
    runtime::{Builder, Runtime},
    sync::Notify,
};

const WARMUP: usize = 1_000;
const ROUNDS: usize = 10_000;
type Spec = [u8; 1024];

#[derive(Default)]
struct Control {
    command: AtomicU8,
    changed: Notify,
}

async fn worker(control: Arc<Control>, context: Context) -> rivium::Result<()> {
    context.mark_progress();
    tokio::select! {
        _ = context.cancel.cancelled() => Ok(()),
        _ = control.changed.notified() => {
            match control.command.swap(0, Ordering::AcqRel) {
                1 => Ok(()),
                2 => Err(rivium::Error::internal("benchmark injected failure")),
                other => panic!("invalid benchmark command {other}"),
            }
        }
    }
}

fn desired(generations: &[u64]) -> Vec<(usize, Spec)> {
    generations
        .iter()
        .enumerate()
        .map(|(key, generation)| {
            let mut spec = [42; 1024];
            spec[..8].copy_from_slice(&generation.to_le_bytes());
            (key, spec)
        })
        .collect()
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    // Nearest rank: ceil(p * count / 100), using a one-based rank.
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

fn run_case(
    runtime: &Runtime,
    label: &str,
    size: usize,
    percent: usize,
    mixed: bool,
    smoke: bool,
    raw: &mut Option<BufWriter<std::fs::File>>,
) {
    let scope = TaskScope::new();
    let mut manager = Manager::with_scope(
        Policy {
            capacity: size,
            max_failures: u32::MAX,
            backoff: Duration::from_nanos(1),
            max_backoff: Duration::from_nanos(1),
            ..Policy::default()
        },
        scope.clone(),
    )
    .unwrap();
    let controls: Vec<_> = (0..size).map(|_| Arc::new(Control::default())).collect();
    let mut generations = vec![0_u64; size];
    let changed = (size * percent).div_ceil(100);
    let warmup = if smoke { 3 } else { WARMUP };
    let rounds = if smoke { 3 } else { ROUNDS };
    let mut samples = Vec::with_capacity(rounds);
    let mut reconcile_calls = Vec::with_capacity(rounds);
    runtime.block_on(async {
        let report = manager
            .reconcile(desired(&generations), |key, _, context| {
                worker(controls[key].clone(), context)
            })
            .await
            .unwrap();
        assert_eq!(report.started.len(), size);
        tokio::task::yield_now().await;

        for round in 0..warmup + rounds {
            let mut exiting = 0;
            let mut replacing = 0;
            for offset in 0..changed {
                let key = (round * changed + offset) % size;
                let action = if mixed { (round + offset) % 3 } else { 2 };
                if action == 2 {
                    generations[key] += 1;
                    replacing += 1;
                } else {
                    controls[key]
                        .command
                        .store((action + 1) as u8, Ordering::Release);
                    controls[key].changed.notify_one();
                    exiting += 1;
                }
            }
            // Scope drain is not a JoinHandle completion barrier on multiple workers.
            let setup_start = Instant::now();
            while scope.active_tasks() != size - exiting {
                assert!(setup_start.elapsed() < Duration::from_secs(5));
                tokio::task::yield_now().await;
            }
            tokio::task::yield_now().await;

            let mut restarted = 0;
            let mut exits = 0;
            let mut calls = 0;
            let start = Instant::now();
            let elapsed = loop {
                let report = manager
                    .reconcile(black_box(desired(&generations)), |key, _, context| {
                        worker(controls[key].clone(), context)
                    })
                    .await
                    .unwrap();
                let elapsed = start.elapsed();
                calls += 1;
                restarted += report.restarted.len();
                exits += report.exits.len();
                assert!(report.unconfirmed.is_empty());
                assert!(report.limited.is_empty());
                assert!(report.exhausted.is_empty());
                black_box(report);
                assert!(restarted <= changed);
                if !mixed || restarted == changed {
                    break elapsed.as_nanos();
                }
                assert!(
                    elapsed < Duration::from_secs(5),
                    "recovery did not converge"
                );
                tokio::task::yield_now().await;
            };

            assert_eq!(restarted, changed);
            assert_eq!(exits, exiting + replacing);
            assert_eq!(manager.len(), size);
            assert_eq!(scope.active_tasks(), size);
            if !mixed {
                assert_eq!(calls, 1);
            }
            if round >= warmup {
                samples.push(elapsed);
                reconcile_calls.push(calls);
            }
            tokio::task::yield_now().await;
        }
        manager.shutdown().await.unwrap();
    });
    assert!(manager.is_empty());
    assert_eq!(scope.active_tasks(), 0);

    let mode = if mixed {
        "mixed_recovery_episode"
    } else {
        "replacement"
    };
    let name = format!("manager/{label}/{mode}/{size}/{percent}pct");
    if let Some(raw) = raw {
        for (round, elapsed) in samples.iter().enumerate() {
            writeln!(raw, "{name},{round},{elapsed},{}", reconcile_calls[round]).unwrap();
        }
        raw.flush().unwrap();
    }
    samples.sort_unstable();
    println!(
        "{name},{size},1024,{percent},{changed},{warmup},{rounds},{},{},{},{},{},{},{},{}",
        percentile(&samples, 50),
        percentile(&samples, 95),
        percentile(&samples, 99),
        samples[0],
        samples[rounds - 1],
        reconcile_calls.iter().sum::<usize>(),
        reconcile_calls.iter().max().unwrap(),
        reconcile_calls.iter().filter(|&&calls| calls > 1).count(),
    );
}

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--test");
    assert_eq!(percentile(&(1..=100).collect::<Vec<_>>(), 99), 99);
    assert_eq!(percentile(&[1, 2, 3], 50), 2);
    let mut raw = std::env::var_os("RIVIUM_LATENCY_RAW").map(|path| {
        let mut output = BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(output, "case,round,elapsed_ns,reconcile_calls").unwrap();
        output
    });
    println!(
        "case,tasks,spec_bytes,change_percent,affected_tasks,warmup,rounds,p50_ns,p95_ns,p99_ns,min_ns,max_ns,total_reconcile_calls,max_reconcile_calls,multiple_call_samples"
    );
    for label in ["current_thread", "2_workers"] {
        let runtime = if label == "current_thread" {
            Builder::new_current_thread().enable_all().build().unwrap()
        } else {
            Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
        };
        for size in [128, 512, 1024] {
            for percent in [0, 1, 10, 100] {
                for mixed in [false, true] {
                    run_case(&runtime, label, size, percent, mixed, smoke, &mut raw);
                }
            }
        }
    }
}
