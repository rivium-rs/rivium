#![cfg(feature = "embedded")]

use rivium::{Error, Kind, Result, Service, service};
use rivium_host::{
    App, Code, Context,
    embedded::{Host, Status},
};
use rivium_test::TempDir;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Default, Serialize, Deserialize)]
struct Config {}

#[derive(Default)]
struct Probe {
    context: Mutex<Option<Context<Config>>>,
    started: AtomicUsize,
    child_started: AtomicBool,
    fail: tokio::sync::Notify,
    released: Mutex<bool>,
    release: Condvar,
}

fn probes() -> &'static Mutex<BTreeMap<PathBuf, Arc<Probe>>> {
    static PROBES: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Probe>>>> = OnceLock::new();
    PROBES.get_or_init(Mutex::default)
}

struct Descendant;

impl App for Descendant {
    const NAME: &'static str = "scope-tests";
    const VERSION: &'static str = "1";
    type Config = Config;

    fn services(_: &Config, context: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        let probe = probes().lock().unwrap()[context.paths().root()].clone();
        *probe.context.lock().unwrap() = Some(context.clone());
        Ok(vec![service(
            "parent",
            Kind::Frontline,
            move |ctx| async move {
                if probe.started.fetch_add(1, Ordering::SeqCst) == 0 {
                    let child = probe.clone();
                    let reservation = ctx
                        .task_scope()
                        .reserve(1)
                        .expect("scope is open while parent starts")
                        .pop()
                        .unwrap();
                    let tracked = reservation.track(async move {
                        child.child_started.store(true, Ordering::SeqCst);
                        let (released, _) = child
                            .release
                            .wait_timeout_while(child.released.lock().unwrap(), WAIT, |v| !*v)
                            .unwrap();
                        assert!(*released, "test did not release descendant");
                    });
                    let runtime = tokio::runtime::Handle::current();
                    // This work outlives parent cancellation and runtime shutdown_timeout.
                    tokio::task::spawn_blocking(move || runtime.block_on(tracked));
                }
                ctx.ready();
                tokio::select! {
                    _ = ctx.stopped() => Ok(()),
                    _ = probe.fail.notified() => Err(Error::internal("parent failed")),
                }
            },
        )])
    }
}

struct Fixture {
    host: Host,
    probe: Arc<Probe>,
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            host: Host::new::<Descendant>(),
            probe: Arc::new(Probe::default()),
            dir: TempDir::new().unwrap(),
        };
        probes()
            .lock()
            .unwrap()
            .insert(fixture.dir.path().to_owned(), fixture.probe.clone());
        fixture
    }

    fn start(&self) {
        assert_eq!(self.host.start(&self.args()), Code::Ok);
        until(|| self.probe.child_started.load(Ordering::SeqCst));
    }

    async fn pending_tasks(&self) -> usize {
        self.host.drain(Duration::ZERO).await.pending.service_tasks
    }

    fn args(&self) -> Vec<String> {
        vec![
            "--root".into(),
            self.dir.path().display().to_string(),
            "--set".into(),
            "lifecycle.stop_timeout=50ms".into(),
        ]
    }

    fn release(&self) {
        *self.probe.released.lock().unwrap() = true;
        self.probe.release.notify_all();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release();
        let _ = self.host.stop(WAIT);
        probes().lock().unwrap().remove(self.dir.path());
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn surviving_descendant_blocks_restart(restart: bool) {
    let fixture = Fixture::new();
    let args = fixture.args();
    fixture.start();
    assert_eq!(fixture.host.active_tasks(), 2);
    assert_eq!(fixture.host.start(&args), Code::AlreadyRunning);
    let context = fixture.probe.context.lock().unwrap().clone().unwrap();
    if restart {
        context.control().restart("test descendant ownership");
    } else {
        fixture.probe.fail.notify_one();
    }

    until(|| fixture.host.status() == Status::Failed);
    assert_eq!(fixture.probe.started.load(Ordering::SeqCst), 1);
    assert_eq!(context.readiness().active_tasks(), 1);
    assert_eq!(fixture.host.active_tasks(), 1);
    assert_eq!(fixture.host.start(&args), Code::Busy);

    fixture.release();
    until(|| context.readiness().active_tasks() == 0);
    assert_eq!(fixture.host.active_tasks(), 0);
    assert_eq!(fixture.host.start(&args), Code::Ok);
    assert_eq!(fixture.probe.started.load(Ordering::SeqCst), 2);
}

#[test]
fn failed_parent_cannot_restart_over_a_surviving_descendant() {
    surviving_descendant_blocks_restart(false);
}

#[test]
fn explicit_restart_cannot_start_over_a_surviving_descendant() {
    surviving_descendant_blocks_restart(true);
}

#[tokio::test]
async fn drain_confirms_release_after_stop_timeout() {
    let fixture = Fixture::new();
    fixture.start();
    assert_eq!(fixture.host.active_tasks(), 2);
    assert_eq!(fixture.pending_tasks().await, 2,);
    assert_eq!(
        fixture.host.stop(Duration::from_secs(2)),
        Code::StopTimedOut
    );
    assert_eq!(fixture.pending_tasks().await, 1,);
    assert_eq!(fixture.host.start(&fixture.args()), Code::Busy);

    fixture.release();
    assert!(fixture.host.drain(WAIT).await.is_success());
    assert_eq!(fixture.host.active_tasks(), 0);
}

#[tokio::test]
async fn drain_before_any_start_is_complete() {
    let host = Host::new::<Descendant>();
    assert_eq!(host.active_tasks(), 0);
    assert!(host.drain(Duration::ZERO).await.is_success());
}

#[tokio::test]
async fn pending_drain_observes_host_completion() {
    let fixture = Fixture::new();
    fixture.start();
    let context = fixture.probe.context.lock().unwrap().clone().unwrap();
    let drain = fixture.host.drain(WAIT);
    tokio::pin!(drain);
    tokio::select! {
        result = &mut drain => panic!("running host unexpectedly drained: {result:?}"),
        _ = tokio::task::yield_now() => {},
    }
    fixture.release();
    context.control().stop(rivium::rt::StopReason::Requested);
    assert!(drain.await.is_success());
    assert_eq!(fixture.host.active_tasks(), 0);
}

/// RIVIUM_STRESS_SECONDS=3600 cargo test -p rivium-host --features embedded \
///     --test scope surviving_descendants_restart_stress -- --ignored --nocapture
#[test]
#[ignore = "opt-in sustained host restart stress; defaults to 60 seconds"]
fn surviving_descendants_restart_stress() {
    let seconds = std::env::var("RIVIUM_STRESS_SECONDS")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("RIVIUM_STRESS_SECONDS must be a u64")
        })
        .unwrap_or(60);
    assert!(seconds > 0, "stress duration must be positive");
    let started = Instant::now();
    let mut generations = 0;
    while generations < 2 || started.elapsed() < Duration::from_secs(seconds) {
        surviving_descendant_blocks_restart(generations % 2 == 0);
        generations += 1;
    }
    eprintln!(
        "host restart stress passed: blocked-and-released generations={generations}, elapsed={:?}",
        started.elapsed()
    );
}
