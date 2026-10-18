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
    future::{Future, poll_fn},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    task::{Poll, Wake, Waker},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    root: PathBuf,
    interval: u64,
    capacity: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            interval: 1,
            capacity: 10,
        }
    }
}

#[derive(Default)]
struct Probe {
    context: Mutex<Option<Context<Config>>>,
    started: AtomicUsize,
    build_attempts: AtomicUsize,
    observed: AtomicU64,
    preparing: AtomicBool,
    released: Mutex<bool>,
    release: Condvar,
    fail: tokio::sync::Notify,
    failure: Mutex<Option<Error>>,
}

impl Probe {
    fn pause(&self) {
        self.preparing.store(true, Ordering::SeqCst);
        let (released, _) = self
            .release
            .wait_timeout_while(self.released.lock().unwrap(), WAIT, |released| !*released)
            .unwrap();
        assert!(*released, "test did not release preparation barrier");
    }
}

fn probes() -> &'static Mutex<BTreeMap<PathBuf, Arc<Probe>>> {
    static PROBES: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Probe>>>> = OnceLock::new();
    PROBES.get_or_init(Mutex::default)
}

fn services(_: &Config, context: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
    let probe = probes().lock().unwrap()[context.paths().root()].clone();
    *probe.context.lock().unwrap() = Some(context.clone());
    let mut config = context.config();
    Ok(vec![service(
        "reload-worker",
        Kind::Frontline,
        move |ctx| async move {
            probe.started.fetch_add(1, Ordering::SeqCst);
            probe
                .observed
                .store(config.snapshot().value.interval, Ordering::SeqCst);
            ctx.ready();
            loop {
                tokio::select! {
                    _ = ctx.stopped() => return Ok(()),
                    _ = probe.fail.notified() => return Err(
                        probe.failure.lock().unwrap().take()
                            .expect("failure set before notification")
                    ),
                    changed = config.changed() => match changed {
                        Some(value) => probe.observed.store(value.interval, Ordering::SeqCst),
                        None => return Ok(()),
                    }
                }
            }
        },
    )])
}

fn validate(config: &Config) -> Result<()> {
    if config.interval == 0 || config.interval > config.capacity {
        return Err(Error::invalid("interval must be in 1..=capacity"));
    }
    Ok(())
}

// All these cases use the same real service; only the declared hooks differ.
macro_rules! reload_app {
    ($name:ident {$($hook:item)*}) => {
        struct $name;

        impl App for $name {
            const NAME: &'static str = "reload-tests";
            const VERSION: &'static str = "1";
            type Config = Config;

            fn services(
                config: &Config,
                context: &Context<Config>
            ) -> Result<Vec<Box<dyn Service>>> {
                services(config, context)
            }

            $($hook)*
        }
    };
}

reload_app! {
    Hot {
        fn validate(config: &Config) -> Result<()> {
            validate(config)
        }

        fn reload_config(current: &Config, candidate: &Config) -> Result<Option<Config>> {
            let mut effective = current.clone();
            effective.interval = candidate.interval;
            Ok(Some(effective))
        }
    }
}

reload_app! { Cold {} }

reload_app! {
    Panicking {
        fn reload_config(_: &Config, _: &Config) -> Result<Option<Config>> {
            panic!("reload policy panicked");
        }
    }
}

reload_app! {
    Blocking {
        fn reload_config(current: &Config, candidate: &Config) -> Result<Option<Config>> {
            let probe = probes().lock().unwrap()[&current.root].clone();
            probe.pause();
            Hot::reload_config(current, candidate)
        }
    }
}

struct Fixture {
    host: Arc<Host>,
    probe: Arc<Probe>,
    dir: TempDir,
}

impl Fixture {
    fn start<A: App<Config = Config>>(sets: &[&str]) -> Self {
        let fixture = Self {
            host: Arc::new(Host::new::<A>()),
            probe: Arc::new(Probe::default()),
            dir: TempDir::new().unwrap(),
        };
        probes()
            .lock()
            .unwrap()
            .insert(fixture.dir.path().to_owned(), fixture.probe.clone());
        fixture.write(1, 10, "");
        let mut args = vec!["--root".into(), fixture.dir.path().display().to_string()];
        for value in sets {
            args.extend(["--set".into(), (*value).into()]);
        }
        assert_eq!(fixture.host.start(&args), Code::Ok);
        fixture
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("reload-tests.toml")
    }

    fn write(&self, interval: u64, capacity: u64, extra: &str) {
        let config = Config {
            root: self.dir.path().into(),
            interval,
            capacity,
        };
        let text = format!("{}\n{extra}", toml::to_string(&config).unwrap());
        std::fs::write(self.path(), text).unwrap();
    }

    fn context(&self) -> Context<Config> {
        self.probe.context.lock().unwrap().clone().unwrap()
    }

    async fn wait_running(&self, generation: usize) {
        until(|| {
            self.probe.started.load(Ordering::SeqCst) == generation
                && self.host.status() == Status::Running
        })
        .await;
    }

    async fn stopped_report(&self, diagnostic: &str) -> rivium_host::HostReport {
        until(|| matches!(self.host.status(), Status::Idle | Status::Failed)).await;
        let report = self.host.last_report().unwrap();
        assert_eq!(report.code, Code::Ok);
        assert!(format!("{:#}", report.primary.as_ref().unwrap()).contains(diagnostic));
        assert_eq!(self.probe.started.load(Ordering::SeqCst), 1);
        report
    }

    fn release(&self) {
        *self.probe.released.lock().unwrap() = true;
        self.probe.release.notify_all();
    }

    fn assert_unchanged(&self) {
        let snapshot = self.context().config().snapshot();
        assert_eq!(snapshot.generation, 0);
        assert_eq!(snapshot.value.interval, 1);
        assert_eq!(snapshot.value.capacity, 10);
        assert_eq!(self.probe.started.load(Ordering::SeqCst), 1);
        assert_eq!(self.host.status(), Status::Running);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release();
        let _ = self.host.stop(WAIT);
        probes().lock().unwrap().remove(self.dir.path());
    }
}

async fn completed<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("test operation timed out")
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("condition did not become true before the deadline");
}

async fn poll_pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn hot_fields_reach_the_same_service_and_cold_fields_keep_running_values() {
    let fixture = Fixture::start::<Hot>(&[]);
    let context = fixture.context();
    let config = context.config();
    fixture.write(
        3,
        20,
        "[log]\nfilter = 'debug'\n[lifecycle]\nstop_timeout = '2s'\n",
    );

    let report = context.reload().await.unwrap();
    assert_eq!(report.published, ["interval"]);
    assert_eq!(report.generation, 1);
    for field in ["capacity", "log.filter", "lifecycle.stop_timeout"] {
        assert!(
            report.requires_restart.iter().any(|key| key == field),
            "missing cold field {field}: {report:?}"
        );
    }

    let snapshot = config.snapshot();
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.value.interval, 3);
    assert_eq!(snapshot.value.capacity, 10);
    until(|| fixture.probe.observed.load(Ordering::SeqCst) == 3).await;
    assert_eq!(fixture.probe.started.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.status(), Status::Running);

    let repeated = fixture.host.reload().await.unwrap();
    assert!(repeated.published.is_empty());
    assert_eq!(repeated.generation, 1);
    assert_eq!(repeated.requires_restart, report.requires_restart);
}

#[tokio::test]
async fn default_policy_keeps_every_application_field_cold() {
    let fixture = Fixture::start::<Cold>(&[]);
    fixture.write(3, 20, "");
    let report = fixture.host.reload().await.unwrap();
    assert!(report.published.is_empty());
    assert_eq!(report.generation, 0);
    assert_eq!(report.requires_restart, ["capacity", "interval"]);
    fixture.assert_unchanged();
}

#[tokio::test]
async fn both_candidate_and_merged_effective_configuration_are_validated() {
    let fixture = Fixture::start::<Hot>(&[]);

    // The candidate is invalid even though retaining capacity would make it valid.
    fixture.write(8, 5, "");
    assert!(fixture.host.reload().await.is_err());
    fixture.assert_unchanged();

    // The candidate is valid, but interval 15 cannot use the retained capacity 10.
    fixture.write(15, 20, "");
    assert!(fixture.context().reload().await.is_err());
    fixture.assert_unchanged();

    fixture.write(2, 10, "");
    assert_eq!(fixture.host.reload().await.unwrap().published, ["interval"]);
    assert_eq!(fixture.context().config().snapshot().generation, 1);
}

#[tokio::test]
async fn missing_or_malformed_file_keeps_the_last_valid_snapshot() {
    let fixture = Fixture::start::<Hot>(&[]);
    std::fs::remove_file(fixture.path()).unwrap();
    assert!(fixture.host.reload().await.is_err());
    fixture.assert_unchanged();

    std::fs::write(fixture.path(), "interval = [").unwrap();
    assert!(fixture.host.reload().await.is_err());
    fixture.assert_unchanged();

    fixture.write(2, 10, "");
    assert_eq!(fixture.host.reload().await.unwrap().generation, 1);
}

#[tokio::test]
async fn rejected_reload_logs_classification_without_configuration_values() {
    let fixture = Fixture::start::<Hot>(&[]);
    let secret = "reload-secret-sentinel";
    std::fs::write(
        fixture.path(),
        format!("password = \"{secret}\" trailing\n"),
    )
    .unwrap();

    let error = fixture.host.reload().await.unwrap_err();
    assert_eq!(error.class(), rivium::error::Class::InvalidInput);
    assert!(error.to_string().contains(secret));
    fixture.assert_unchanged();

    assert_eq!(fixture.host.stop(WAIT), Code::Ok);
    let log = std::fs::read_to_string(
        fixture
            .dir
            .path()
            .join("logs/reload-tests/reload-tests.log"),
    )
    .unwrap();
    assert!(log.contains("configuration reload rejected"));
    assert!(!log.contains(secret));
    assert!(log.contains("InvalidInput"));
}

#[tokio::test]
async fn no_op_and_concurrent_reloads_do_not_publish_duplicate_generations() {
    let fixture = Fixture::start::<Hot>(&[]);
    let report = fixture.host.reload().await.unwrap();
    assert!(report.published.is_empty());
    assert_eq!(report.generation, 0);
    assert!(report.requires_restart.is_empty());

    fixture.write(3, 10, "");
    let context = fixture.context();
    let (first, second) = tokio::join!(fixture.host.reload(), context.reload());
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.published.len() + second.published.len(), 1);
    assert!(first.published == ["interval"] || second.published == ["interval"]);
    assert_eq!(first.generation, 1);
    assert_eq!(second.generation, 1);
    assert_eq!(fixture.context().config().snapshot().generation, 1);
}

#[tokio::test]
async fn reload_retains_startup_cli_overrides() {
    let fixture = Fixture::start::<Hot>(&["interval=4"]);
    fixture.write(7, 10, "");
    let report = fixture.host.reload().await.unwrap();
    assert!(report.published.is_empty());
    assert_eq!(report.generation, 0);
    assert!(report.requires_restart.is_empty());
    assert_eq!(fixture.context().config().snapshot().value.interval, 4);
}

#[tokio::test]
async fn decision_panics_are_contained_and_preserve_the_running_host() {
    let fixture = Fixture::start::<Panicking>(&[]);
    fixture.write(2, 10, "");
    assert!(fixture.host.reload().await.is_err());
    fixture.assert_unchanged();
    assert!(fixture.context().reload().await.is_err());
    fixture.assert_unchanged();
}

#[tokio::test]
async fn restart_closes_old_contexts_and_configuration_subscriptions() {
    let fixture = Fixture::start::<Hot>(&[]);
    let old = fixture.context();
    let mut old_config = old.config();
    fixture.write(3, 10, "");
    old.control().restart("test configuration incarnation");
    fixture.wait_running(2).await;

    assert!(old.reload().await.is_err());
    assert!(completed(old_config.changed()).await.is_none());
    assert_eq!(old_config.snapshot().value.interval, 1);
    let new = fixture.context();
    assert_eq!(new.config().snapshot().value.interval, 3);
    assert_eq!(new.config().snapshot().generation, 0);
    assert!(new.reload().await.unwrap().published.is_empty());
}

#[tokio::test]
async fn stopping_rejects_in_flight_preparation_and_later_reload_requests() {
    let fixture = Fixture::start::<Blocking>(&[]);
    let context = fixture.context();
    let mut config = context.config();
    fixture.write(3, 10, "");
    let host = fixture.host.clone();
    let pending = tokio::spawn(async move { host.reload().await });
    until(|| fixture.probe.preparing.load(Ordering::SeqCst)).await;

    let stop_started = std::time::Instant::now();
    assert_eq!(
        fixture.host.stop(Duration::from_millis(200)),
        Code::StopTimedOut
    );
    assert!(
        stop_started.elapsed() < Duration::from_secs(1),
        "stop added a cleanup budget after the caller budget"
    );
    assert!(completed(pending).await.unwrap().is_err());
    assert!(fixture.host.reload().await.is_err());
    assert!(context.reload().await.is_err());
    assert_eq!(config.snapshot().generation, 0);
    assert_eq!(config.snapshot().value.interval, 1);
    let args = [
        "--root".to_string(),
        fixture.dir.path().display().to_string(),
    ];
    assert_eq!(fixture.host.start(&args), Code::Busy);
    fixture.release();
    completed(async {
        loop {
            match fixture.host.start(&args) {
                Code::Ok => break,
                Code::Busy => tokio::task::yield_now().await,
                code => panic!("unexpected restart result: {code:?}"),
            }
        }
    })
    .await;
    assert_eq!(fixture.context().config().snapshot().generation, 0);
    assert_eq!(fixture.context().config().snapshot().value.interval, 3);
    assert!(completed(config.changed()).await.is_none());
}

#[tokio::test]
async fn full_reload_queue_rejects_excess_requests_and_recovers_when_drained() {
    let fixture = Fixture::start::<Blocking>(&[]);
    let context = fixture.context();
    fixture.write(3, 10, "");
    let mut preparing = Box::pin(context.reload());
    poll_pending(preparing.as_mut()).await;
    until(|| fixture.probe.preparing.load(Ordering::SeqCst)).await;

    let mut queued = Vec::new();
    for _ in 0..8 {
        let mut request = Box::pin(context.reload());
        poll_pending(request.as_mut()).await;
        queued.push(request);
    }

    let error = tokio::time::timeout(WAIT, fixture.host.reload())
        .await
        .expect("full queue must reject without waiting for preparation")
        .unwrap_err();
    assert_eq!(error.class(), rivium::error::Class::Conflict);
    fixture.assert_unchanged();

    fixture.release();
    let first = completed(preparing).await.unwrap();
    assert_eq!(first.published, ["interval"]);
    for request in queued {
        let report = completed(request).await.unwrap();
        assert_eq!(report.generation, 1);
        assert!(report.published.is_empty());
    }
    assert_eq!(context.config().snapshot().value.interval, 3);
}

#[tokio::test]
async fn cancelling_reload_waiter_discards_prepared_result_and_allows_next_publication() {
    let fixture = Fixture::start::<Blocking>(&[]);
    let context = fixture.context();
    fixture.write(3, 10, "");
    let mut cancelled = Box::pin(context.reload());
    poll_pending(cancelled.as_mut()).await;
    until(|| fixture.probe.preparing.load(Ordering::SeqCst)).await;

    drop(cancelled);
    fixture.assert_unchanged();
    fixture.release();
    let recovered = completed(fixture.host.reload()).await.unwrap();
    assert_eq!(recovered.published, ["interval"]);
    assert_eq!(recovered.generation, 1);
    assert_eq!(context.config().snapshot().value.interval, 3);
    assert_eq!(fixture.probe.started.load(Ordering::SeqCst), 1);
}

// Probe public reentry from another thread so a regression fails instead of deadlocking
// the test runner. A synchronous reentry would wait on the same lock indefinitely.
type WakeResult = (bool, std::thread::JoinHandle<()>);

struct ReentrantWake {
    action: Arc<dyn Fn() + Send + Sync>,
    result: Mutex<Option<tokio::sync::oneshot::Sender<WakeResult>>>,
    inner: Waker,
}

impl Wake for ReentrantWake {
    fn wake(self: Arc<Self>) {
        let result = self.result.lock().unwrap().take();
        if let Some(result) = result {
            let action = self.action.clone();
            let (finished, observed) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                action();
                let _ = finished.send(());
            });
            let reentered = observed.recv_timeout(Duration::from_secs(1)).is_ok();
            let _ = result.send((reentered, thread));
        }
        self.inner.wake_by_ref();
    }
}

async fn observe_wake<F: Future>(
    mut future: Pin<&mut F>,
    action: impl Fn() + Send + Sync + 'static,
) -> tokio::sync::oneshot::Receiver<WakeResult> {
    let (result, observed) = tokio::sync::oneshot::channel();
    let mut result = Some(result);
    let action = Arc::new(action);
    poll_fn(|cx| {
        let waker = Waker::from(Arc::new(ReentrantWake {
            action: action.clone(),
            result: Mutex::new(result.take()),
            inner: cx.waker().clone(),
        }));
        assert!(
            future
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker))
                .is_pending()
        );
        Poll::Ready(())
    })
    .await;
    observed
}

async fn assert_unlocked_callback(observed: tokio::sync::oneshot::Receiver<WakeResult>) {
    let (reentered, thread) = completed(observed).await.unwrap();
    thread.join().unwrap();
    assert!(
        reentered,
        "callback could not reenter host before the deadline"
    );
}

#[tokio::test]
async fn completion_waker_can_query_host_status() {
    let fixture = Fixture::start::<Hot>(&[]);
    let mut drain = Box::pin(fixture.host.drain(WAIT));
    let host = fixture.host.clone();
    let observed = observe_wake(drain.as_mut(), move || {
        assert_eq!(host.status(), Status::Idle);
    })
    .await;
    let host = fixture.host.clone();
    let stopped = tokio::task::spawn_blocking(move || host.stop(WAIT));
    assert_unlocked_callback(observed).await;
    assert_eq!(stopped.await.unwrap(), Code::Ok);
    assert!(drain.await.is_success());
}

#[tokio::test]
async fn configuration_waker_can_read_and_request_reload() {
    let fixture = Fixture::start::<Hot>(&[]);
    let context = fixture.context();
    let mut config = context.config();
    let mut changed = Box::pin(config.changed());
    let reentrant = context.clone();
    let observed = observe_wake(changed.as_mut(), move || {
        assert_eq!(reentrant.config().snapshot().value.interval, 3);
        let mut request = Box::pin(reentrant.reload());
        let result = request
            .as_mut()
            .poll(&mut std::task::Context::from_waker(Waker::noop()));
        assert!(matches!(result, Poll::Pending | Poll::Ready(Ok(_))));
    })
    .await;
    fixture.write(3, 10, "");
    assert_eq!(context.reload().await.unwrap().generation, 1);
    assert_unlocked_callback(observed).await;
    assert_eq!(changed.await.unwrap().interval, 3);
}

#[tokio::test]
async fn two_restarts_rebuild_loggers_and_apply_cold_settings() {
    let fixture = Fixture::start::<Hot>(&[]);
    for (generation, directory) in [(2, "logs/second"), (3, "logs/third")] {
        let old = fixture.context();
        fixture.write(
            generation as u64,
            10,
            &format!("[log.file]\ndir='{directory}'\n[lifecycle]\nstop_timeout='100ms'\n"),
        );
        let report = old.reload().await.unwrap();
        assert!(
            report
                .requires_restart
                .iter()
                .any(|field| field == "log.file.dir")
        );
        old.control().restart("apply cold fields");
        fixture.wait_running(generation).await;
        assert!(old.reload().await.is_err());
        assert!(fixture.dir.path().join(directory).is_dir());
        let fresh = fixture.context();
        assert_eq!(fresh.config().snapshot().generation, 0);
        assert_eq!(fresh.config().current().interval, generation as u64);
        assert!(fresh.reload().await.unwrap().requires_restart.is_empty());
    }
}

#[tokio::test]
async fn drain_is_bound_to_the_generation_captured_before_restart() {
    let fixture = Fixture::start::<Hot>(&[]);
    let old = fixture.context();
    let mut draining = Box::pin(fixture.host.drain(WAIT));
    poll_pending(draining.as_mut()).await;
    old.control().restart("replace observed generation");
    fixture.wait_running(2).await;
    let cleanup = tokio::time::timeout(Duration::from_millis(300), draining)
        .await
        .expect("old generation drain followed the replacement generation");
    assert!(cleanup.is_success());
    assert_eq!(fixture.host.status(), Status::Running);
    assert_eq!(fixture.host.active_tasks(), 1);
}

#[tokio::test]
async fn reload_worker_panic_is_retained_as_a_cleanup_failure() {
    struct PanicOnWarning;

    impl tracing_subscriber::Layer<tracing_subscriber::Registry> for PanicOnWarning {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, tracing_subscriber::Registry>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                panic!("private reload log failure");
            }
        }
    }

    reload_app! {
        AppWithLayer {
            fn log_layers() -> Vec<rivium_host::log::ExtraLayer> {
                vec![Box::new(PanicOnWarning)]
            }
        }
    }

    let fixture = Fixture::start::<AppWithLayer>(&[]);
    std::fs::write(fixture.path(), "invalid = [").unwrap();
    assert!(fixture.host.reload().await.is_err());
    assert_eq!(fixture.host.stop(WAIT), Code::Fault);
    let report = fixture.host.last_report().unwrap();
    assert!(report.cleanup.is_drained());
    assert!(
        report
            .cleanup
            .failures
            .iter()
            .any(|failure| failure.stage == rivium_host::CleanupStage::Reload)
    );
}

#[tokio::test]
async fn drain_captures_restart_initialization_before_resources_exist() {
    reload_app! {
        BlockInitialization {
            fn validate(config: &Config) -> Result<()> {
                if config.interval == 2 {
                    let probe = probes().lock().unwrap()[&config.root].clone();
                    probe.pause();
                    return Err(Error::invalid("initialization rejected after barrier"));
                }
                Ok(())
            }
        }
    }

    for stop in [false, true] {
        let fixture = Fixture::start::<BlockInitialization>(&[]);
        let old = fixture.context();
        fixture.write(2, 10, "");
        old.control().restart("new initialization");
        until(|| fixture.probe.preparing.load(Ordering::SeqCst)).await;
        let initial = fixture.host.drain(Duration::ZERO).await;
        assert!(
            !initial.closed,
            "drain returned the previous generation's completion"
        );
        assert_eq!(initial.pending, rivium_host::PendingResources::default());
        let mut drain = Box::pin(fixture.host.drain(WAIT));
        poll_pending(drain.as_mut()).await;
        if stop {
            assert_eq!(fixture.host.stop(Duration::ZERO), Code::StopTimedOut);
        } else {
            fixture.write(3, 10, "");
        }
        fixture.release();
        assert!(drain.await.is_success());
        if stop {
            fixture.stopped_report("initialization rejected").await;
        } else {
            fixture.wait_running(2).await;
            assert_eq!(fixture.host.last_report().unwrap().code, Code::Config);
        }
    }
}

struct DocumentApp;

impl App for DocumentApp {
    const NAME: &'static str = "document";
    const VERSION: &'static str = "test";
    type Config = BTreeMap<String, toml::Value>;

    fn services(_: &Self::Config, _: &Context<Self::Config>) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![rivium_test::idle("worker", Kind::Frontline)])
    }

    fn reload_config(_: &Self::Config, candidate: &Self::Config) -> Result<Option<Self::Config>> {
        Ok(Some(candidate.clone()))
    }
}

fn document_fixture(text: &str) -> (TempDir, Host) {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("document.toml"), text).unwrap();
    let host = Host::new::<DocumentApp>();
    assert_eq!(
        host.start(&["--root".into(), root.path().display().to_string()]),
        Code::Ok
    );
    (root, host)
}

#[tokio::test]
async fn unchanged_nan_values_do_not_create_a_reload() {
    let (_root, host) = document_fixture("amount=nan\n");
    let report = host.reload().await.unwrap();
    let stopped = host.stop(WAIT);
    assert_eq!(stopped, Code::Ok);
    assert_eq!(report.generation, 0);
    assert!(report.published.is_empty());
    assert!(report.requires_restart.is_empty());
}

#[tokio::test]
async fn a_later_generation_retries_transient_construction_errors_and_panics() {
    struct Rebuild<const PANIC: bool>;

    impl<const PANIC: bool> App for Rebuild<PANIC> {
        const NAME: &'static str = Hot::NAME;
        const VERSION: &'static str = Hot::VERSION;
        type Config = Config;

        fn services(config: &Config, context: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
            let probe = probes().lock().unwrap()[context.paths().root()].clone();
            if probe.build_attempts.fetch_add(1, Ordering::SeqCst) == 1 {
                if PANIC {
                    panic!("one transient composition panic");
                }
                return Err(Error::internal("one transient composition failure"));
            }
            services(config, context)
        }
    }

    async fn verify<const PANIC: bool>() {
        let fixture = Fixture::start::<Rebuild<PANIC>>(&[]);
        fixture.write(3, 10, "");
        fixture
            .context()
            .control()
            .restart("retry the rebuilt generation");
        fixture.wait_running(2).await;
        assert_eq!(fixture.probe.build_attempts.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.context().config().current().interval, 3);
        assert_eq!(
            fixture.host.last_report().unwrap().code,
            Code::StartupFailed
        );
    }
    verify::<false>().await;
    verify::<true>().await;
}

#[tokio::test]
async fn diff_reports_added_removed_and_array_fields() {
    let (root, host) = document_fixture("items=[1,2]\n[map]\nremoved=1\nkept=2\n");
    let file = root.path().join("document.toml");
    std::fs::write(file, "items=[2,3]\n[map]\nadded=3\nkept=2\n").unwrap();
    let report = host.reload().await.unwrap();
    assert_eq!(report.published, ["items", "map.added", "map.removed"]);
    assert_eq!(host.stop(WAIT), Code::Ok);
}

#[tokio::test]
async fn a_cloned_context_checks_writes_and_restarts_its_generation() {
    let fixture = Fixture::start::<Hot>(&[]);
    let context = fixture.context().clone();
    assert!(context.check_config("interval=0").is_err());
    let config = Config {
        root: fixture.dir.path().into(),
        interval: 4,
        capacity: 10,
    };
    let text = toml::to_string(&config).unwrap();
    context.check_config(&text).unwrap();
    context.write_config(&text).unwrap();
    assert_eq!(
        context.config().current().interval,
        1,
        "writing must not publish"
    );
    context.control().restart("apply saved configuration");
    fixture.wait_running(2).await;
    assert_eq!(fixture.context().config().current().interval, 4);
    assert!(context.reload().await.is_err());
}

#[tokio::test]
async fn retired_failure_source_can_query_host_during_final_report_replacement() {
    struct Cause(Arc<ReentrantWake>);

    impl std::fmt::Debug for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("reentrant cause")
        }
    }

    impl std::fmt::Display for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Debug::fmt(self, f)
        }
    }

    impl std::error::Error for Cause {}

    impl Drop for Cause {
        fn drop(&mut self) {
            self.0.clone().wake();
        }
    }

    let fixture = Fixture::start::<Hot>(&[]);
    let host = Arc::downgrade(&fixture.host);
    let (result, observed) = tokio::sync::oneshot::channel();
    let source = Cause(Arc::new(ReentrantWake {
        action: Arc::new(move || assert_eq!(host.upgrade().unwrap().status(), Status::Idle)),
        result: Mutex::new(Some(result)),
        inner: Waker::noop().clone(),
    }));
    *fixture.probe.failure.lock().unwrap() =
        Some(Error::internal("first attempt failed").with_source(source));
    fixture.probe.fail.notify_one();
    fixture.wait_running(2).await;
    assert_eq!(fixture.host.stop(WAIT), Code::Ok);
    assert_unlocked_callback(observed).await;
}

#[tokio::test]
async fn stopping_a_parked_retry_preserves_the_failed_attempt() {
    let fixture = Fixture::start::<Hot>(&[]);
    let mut drain = Box::pin(fixture.host.drain(WAIT));
    let host = fixture.host.clone();
    let observed = observe_wake(drain.as_mut(), move || {
        assert_eq!(host.status(), Status::Restarting);
        assert_eq!(host.stop(Duration::ZERO), Code::StopTimedOut);
    })
    .await;
    *fixture.probe.failure.lock().unwrap() = Some(Error::internal("parked failure"));
    fixture.probe.fail.notify_one();
    assert_unlocked_callback(observed).await;
    assert!(drain.await.is_success());
    let report = fixture.stopped_report("parked failure").await;
    assert!(matches!(
        report.run.unwrap().outcome,
        rivium::Outcome::Failed(_)
    ));
}
