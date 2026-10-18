#![cfg(feature = "embedded")]

use rivium::{Kind, Result, Service};
use rivium_host::{App, Code, Context, embedded::Host};
use rivium_test::TempDir;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

// Only the repeated App adapter; each service body and hook stays explicit.
macro_rules! fixture_app {
    ($name:ident, $id:expr, $config:ty, |$cfg:pat_param, $cx:pat_param| $body:block $($hook:item)*) => {
        struct $name;

        impl App for $name {
            const NAME: &'static str = $id;
            const VERSION: &'static str = "test";
            type Config = $config;

            fn services(
                $cfg: &Self::Config,
                $cx: &Context<Self::Config>,
            ) -> Result<Vec<Box<dyn Service>>> $body

            $($hook)*
        }
    };
}

fn setup<A: App>() -> (TempDir, Arc<Host>, Vec<String>) {
    let root = TempDir::new().unwrap();
    let args = vec!["--root".into(), root.path().display().to_string()];
    (root, Arc::new(Host::new::<A>()), args)
}

fn failed_start<A: App>() -> rivium_host::HostReport {
    let (_root, host, args) = setup::<A>();
    let (sender, receiver) = std::sync::mpsc::channel();
    let started = host.clone();
    let worker = std::thread::spawn(move || {
        let _ = sender.send(started.start(&args));
    });
    let result = receiver.recv_timeout(Duration::from_secs(2));
    assert!(result.is_ok(), "driver panic left start waiting forever");
    assert_eq!(result.unwrap(), Code::Fault);
    worker.join().unwrap();
    host.last_report().unwrap()
}

#[derive(Default, Serialize, Deserialize)]
struct Config {}

fixture_app! {
    Basic, "host-basic", Config, |_, _| {
        Ok(vec![rivium_test::idle("worker", Kind::Frontline)])
    }
}

#[test]
fn embedded_host_reports_ready_and_confirmed_cleanup() {
    let (_root, host, args) = setup::<Basic>();
    assert_eq!(host.start(&args), Code::Ok);
    assert_eq!(host.start(&args), Code::AlreadyRunning);
    assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
    assert!(host.last_report().unwrap().cleanup.is_success());
    assert_eq!(host.start(&args), Code::Ok);
    assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
    assert_eq!(host.stop(Duration::ZERO), Code::NotRunning);
}

#[test]
fn embedded_rejects_process_inspection_without_creating_directories() {
    let (root, host, _) = setup::<Basic>();
    let absent = root.path().join("absent");
    for action in ["--help", "--version", "default-config", "check-config"] {
        let args = [action.into(), "--root".into(), absent.display().to_string()];
        assert_eq!(host.start(&args), Code::Usage);
        assert_eq!(host.last_report().unwrap().code, Code::Usage);
        assert!(host.last_error().is_some());
        assert!(!absent.exists());
    }
}

#[test]
fn stopping_before_readiness_returns_cancelled_to_the_start_caller() {
    fixture_app! {
        Waiting, "waiting-start", Config, |_, _| {
            Ok(vec![rivium::service(
                "initializing",
                Kind::Frontline,
                |context| async move {
                    context.stopped().await;
                    Ok(())
                },
            )])
        }
    }

    let (_root, host, args) = setup::<Waiting>();
    let start_host = host.clone();
    let starting = std::thread::spawn(move || start_host.start(&args));
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while host.active_tasks() != 1 {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(host.stop(Duration::from_secs(1)), Code::Ok);
    assert_eq!(starting.join().unwrap(), Code::Cancelled);
}

#[test]
fn oversized_input_keeps_primary_classification_and_creates_no_logger() {
    let (root, host, args) = setup::<Basic>();
    std::fs::File::create(root.path().join("host-basic.toml"))
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    assert_eq!(host.start(&args), Code::Config);
    let report = host.last_report().unwrap();
    assert_eq!(
        report.primary.unwrap().class(),
        rivium::error::Class::PayloadTooLarge
    );
    assert!(report.run.is_none());
    assert!(report.cleanup.is_success());
    assert!(!root.path().join("logs").exists());
    assert_eq!(host.stop(Duration::ZERO), Code::NotRunning);
}

#[test]
fn host_reserved_sections_cannot_be_application_fields() {
    #[derive(Default, Serialize, Deserialize)]
    struct Reserved {
        log: String,
    }

    fixture_app! {
        Invalid, "reserved-config", Reserved, |_, _| {
            panic!("invalid config must not construct services")
        }
    }

    let (root, host, args) = setup::<Invalid>();
    assert_eq!(host.start(&args), Code::Config);
    assert!(!root.path().join("logs").exists());
}

#[test]
fn logger_cleanup_failure_is_retained_alongside_the_primary_failure() {
    struct Noisy<const FAIL: bool>;

    impl<const FAIL: bool> App for Noisy<FAIL> {
        const NAME: &'static str = "noisy-host";
        const VERSION: &'static str = "test";
        type Config = Config;

        fn services(_: &Config, _: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
            Ok(vec![rivium::service(
                "worker",
                Kind::Frontline,
                |context| async move {
                    tracing::info!("{}", "x".repeat(2048));
                    context.ready();
                    if FAIL {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        return Err(rivium::Error::internal("private-business-failure"));
                    }
                    context.stopped().await;
                    Ok(())
                },
            )])
        }
    }

    fn verify<const FAIL: bool>() {
        let (_root, host, mut args) = setup::<Noisy<FAIL>>();
        args.extend(["--set".into(), "log.file.max_file_size=1024".into()]);
        assert_eq!(host.start(&args), Code::Ok);
        if FAIL {
            let end = std::time::Instant::now() + Duration::from_secs(3);
            while host.status() != rivium_host::embedded::Status::Failed {
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
        } else {
            assert_eq!(host.stop(Duration::from_secs(2)), Code::Fault);
        }

        for _ in 0..2 {
            let report = host.last_report().unwrap();
            assert_eq!(report.code, Code::Fault);
            assert!(report.cleanup.is_drained());
            assert!(!report.cleanup.is_success());
            assert_eq!(report.cleanup.failures.len(), 1);
            assert_eq!(
                report.cleanup.failures[0].stage,
                rivium_host::CleanupStage::Logger
            );
            assert_eq!(report.primary.is_some(), FAIL);
            if let Some(primary) = report.primary {
                assert!(format!("{primary:#}").contains("private-business-failure"));
            }
        }
        assert!(
            !host
                .last_error()
                .unwrap()
                .contains("private-business-failure")
        );
    }
    verify::<false>();
    verify::<true>();
}

#[test]
fn service_readiness_completes_start_even_when_health_is_unknown() {
    fixture_app! {
        Degraded, "degraded-host", Config, |_, context| {
            context.health().register("dependency")?;
            Ok(vec![rivium_test::idle("ready-service", Kind::Frontline)])
        }
    }

    let (_root, host, args) = setup::<Degraded>();
    let (sender, receiver) = std::sync::mpsc::channel();
    let start_host = host.clone();
    let thread = std::thread::spawn(move || sender.send(start_host.start(&args)).unwrap());
    let result = receiver.recv_timeout(Duration::from_millis(500));
    let _ = host.stop(Duration::from_secs(1));
    thread.join().unwrap();
    assert_eq!(result.unwrap(), Code::Ok);
}

#[test]
fn subscriber_panic_finishes_start_and_preserves_cleanup_ownership() {
    struct PanickingLayer;

    impl tracing_subscriber::Layer<tracing_subscriber::Registry> for PanickingLayer {
        fn on_event(
            &self,
            _: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, tracing_subscriber::Registry>,
        ) {
            panic!("private subscriber callback failure");
        }
    }

    fixture_app! {
        Panicking, "panic-layer-host", Config, |_, _| {
            Ok(vec![rivium_test::idle("worker", Kind::Frontline)])
        }

        fn log_layers() -> Vec<rivium_host::log::ExtraLayer> {
            vec![Box::new(PanickingLayer)]
        }
    }

    let report = failed_start::<Panicking>();
    assert!(report.primary.is_some());
    assert!(report.cleanup.is_drained());
}

#[test]
fn supervisor_span_panic_returns_fault_with_a_closed_scope() {
    struct SpanPanic;

    impl tracing_subscriber::Layer<tracing_subscriber::Registry> for SpanPanic {
        fn on_new_span(
            &self,
            attributes: &tracing::span::Attributes<'_>,
            _: &tracing::span::Id,
            _: tracing_subscriber::layer::Context<'_, tracing_subscriber::Registry>,
        ) {
            if attributes.metadata().name() == "service" {
                panic!("private service span failure");
            }
        }
    }

    fixture_app! {
        Panicking, "panic-span-host", Config, |_, _| {
            Ok(vec![rivium_test::idle("worker", Kind::Frontline)])
        }

        fn log_layers() -> Vec<rivium_host::log::ExtraLayer> {
            vec![Box::new(SpanPanic)]
        }
    }

    let report = failed_start::<Panicking>();
    assert!(report.run.unwrap().drain.is_complete());
    assert!(report.cleanup.is_drained());
    assert!(
        report
            .cleanup
            .failures
            .iter()
            .any(|failure| failure.stage == rivium_host::CleanupStage::Runtime)
    );
}

#[test]
fn an_explicit_missing_config_is_rejected_and_reserved_settings_are_validated() {
    let (root, host, base) = setup::<Basic>();
    let mut explicit = base.to_vec();
    explicit.extend(["--config".into(), "missing.toml".into()]);
    assert_eq!(host.start(&explicit), Code::Config);
    assert!(!root.path().join("logs").exists());
    for invalid in [
        "lifecycle.stop_timeout=0s",
        "log.file.max_file_size=1023",
        "log.file.max_total_size=invalid",
    ] {
        let mut args = base.to_vec();
        args.extend(["--set".into(), invalid.into()]);
        assert_eq!(host.start(&args), Code::Config, "{invalid}");
    }
    assert_eq!(host.start(&base), Code::Ok);
    assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
}

#[tokio::test]
async fn one_stop_during_service_construction_is_not_a_second_stop() {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    #[derive(Clone, Default, Serialize, Deserialize)]
    struct Value {
        value: u16,
    }

    struct Gate {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    type Build = (Arc<Gate>, Option<Context<Value>>);
    static BUILD: Mutex<Option<Build>> = Mutex::new(None);

    fn pause() {
        let gate = BUILD.lock().unwrap().as_ref().unwrap().0.clone();
        gate.entered.send(()).unwrap();
        gate.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
    }

    struct PauseFailure(AtomicBool);

    impl tracing_subscriber::Layer<tracing_subscriber::Registry> for PauseFailure {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, tracing_subscriber::Registry>,
        ) {
            let running_pause = BUILD
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|build| build.1.as_ref())
                .is_some_and(|context| context.config().current().value == 3);
            if (*event.metadata().level() == tracing::Level::ERROR
                || (event.metadata().target() == "rivium_host::session" && running_pause))
                && !self.0.swap(true, Ordering::SeqCst)
            {
                pause();
            }
        }
    }

    fixture_app! {
        Building, "stop-construction", Value, |value, context| {
            BUILD.lock().unwrap().as_mut().unwrap().1 = Some(context.clone());
            if value.value >= 2 {
                let fail = value.value == 2;
                return Ok(vec![rivium::service(
                    "worker",
                    Kind::Frontline,
                    move |cx| async move {
                        cx.ready();
                        if fail {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            return Err(rivium::Error::internal("recoverable failure"));
                        }
                        cx.stopped().await;
                        Ok(())
                    },
                )]);
            }
            pause();
            Ok(vec![rivium_test::idle("worker", Kind::Frontline)])
        }

        fn reload_config(_: &Value, candidate: &Value) -> Result<Option<Value>> {
            Ok(Some(candidate.clone()))
        }

        fn log_layers() -> Vec<rivium_host::log::ExtraLayer> {
            vec![Box::new(PauseFailure(AtomicBool::new(false)))]
        }
    }

    for mode in 0..4 {
        let (root, host, mut args) = setup::<Building>();
        let (entered, observed) = mpsc::channel();
        let (release, continued) = mpsc::channel();
        *BUILD.lock().unwrap() = Some((
            Arc::new(Gate {
                entered,
                release: Mutex::new(continued),
            }),
            None,
        ));
        if mode >= 2 {
            args.extend(["--set".into(), format!("value={mode}")]);
        }

        let started = host.clone();
        let worker = std::thread::spawn(move || started.start(&args));
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let context = BUILD.lock().unwrap().as_ref().unwrap().1.clone().unwrap();
        let stopping = if mode == 2 {
            let stopping = host.clone();
            let thread = std::thread::spawn(move || stopping.stop(Duration::from_secs(2)));
            let end = std::time::Instant::now() + Duration::from_secs(2);
            while host.status() != rivium_host::embedded::Status::Stopping {
                assert!(std::time::Instant::now() < end);
                std::thread::yield_now();
            }
            Some(thread)
        } else {
            if mode == 1 {
                let control: &rivium_host::HostControl = context.control();
                control.stop(rivium::StopReason::Requested);
            } else {
                assert_eq!(host.stop(Duration::ZERO), Code::StopTimedOut);
            }
            None
        };
        if mode == 3 {
            assert!(
                !context.readiness().is_ready(),
                "stop must revoke readiness before the driver resumes"
            );
        }
        std::fs::write(root.path().join("stop-construction.toml"), "value=1").unwrap();
        let reload = if mode == 2 {
            None
        } else {
            Some(context.reload().await)
        };
        release.send(()).unwrap();
        let _ = worker.join().unwrap();
        if let Some(stopping) = stopping {
            assert_eq!(stopping.join().unwrap(), Code::Ok);
        }
        assert!(host.drain(Duration::from_secs(2)).await.is_drained());
        if mode == 2 {
            // The failure event held the driver before cleanup and the retry decision.
            let report = host.last_report().unwrap();
            assert_eq!(report.code, Code::Ok, "stop must cancel the eligible retry");
            assert!(
                report.primary.is_some(),
                "the failed attempt's diagnostic must remain"
            );
        }
        if let Some(reload) = reload {
            assert!(
                reload.is_err(),
                "stopped generation published configuration"
            );
        }
        assert_eq!(context.config().generation(), 0);
        assert!(
            !matches!(
                host.last_report().unwrap().run.unwrap().outcome,
                rivium::Outcome::Aborted(_)
            ),
            "single stop delivered twice"
        );
    }
}
