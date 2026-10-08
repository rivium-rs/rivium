//! The embedded host end to end (V-5): every call in every status it can reach in a few
//! seconds, the results of the first round, restarts in the process, concurrent calls, repeated
//! starts and stops, the logs and the threads. One test runs every scenario in order, because a
//! process installs logging once, below the root of its first start. Giving up after ten
//! failures in a row takes four minutes of backoff: `src/embedded/tests.rs` drives that state.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rivium::embedded::{Host, Status};
use rivium::error::{Error, kinds};
use rivium::{App, AppContext, Code, Result, Service, ServiceKind};
use rivium_test::{ScriptedService, Step};
use serde::{Deserialize, Serialize};

use ServiceKind::{Background, Frontline};

/// What the next rounds build; the last one repeats.
static ROUNDS: Mutex<Vec<Build>> = Mutex::new(Vec::new());
/// How many rounds have been built.
static BUILT: Mutex<usize> = Mutex::new(0);

type Build = fn() -> Result<Vec<Box<dyn Service>>>;

struct Probe;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

impl App for Probe {
    const NAME: &'static str = "probe";
    const VERSION: &'static str = "1.0.0";
    type Config = Config;

    fn services(_: &Config, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        *BUILT.lock().unwrap() += 1;
        let build = {
            let mut rounds = ROUNDS.lock().unwrap();
            if rounds.len() > 1 {
                rounds.remove(0)
            } else {
                rounds[0]
            }
        };
        build()
    }
}

fn scripted(kind: ServiceKind, steps: Vec<Step>) -> Result<Vec<Box<dyn Service>>> {
    Ok(vec![Box::new(ScriptedService::new("http", kind, steps))])
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn serving() -> Result<Vec<Box<dyn Service>>> {
    scripted(Frontline, vec![Step::Ready, Step::UntilStopped])
}

fn slow_to_start() -> Result<Vec<Box<dyn Service>>> {
    scripted(
        Frontline,
        vec![Step::Sleep(ms(1_500)), Step::Ready, Step::UntilStopped],
    )
}

/// Ready after half a second: a rebuilt round is starting for a while.
fn ready_soon() -> Result<Vec<Box<dyn Service>>> {
    scripted(
        Frontline,
        vec![Step::Sleep(ms(500)), Step::Ready, Step::UntilStopped],
    )
}

fn faulting() -> Result<Vec<Box<dyn Service>>> {
    scripted(
        Frontline,
        vec![Step::Ready, Step::Sleep(ms(300)), Step::Fail("device gone")],
    )
}

fn restarting() -> Result<Vec<Box<dyn Service>>> {
    let service = rivium::service("config", Background, |ctx| async move {
        ctx.ready();
        tokio::time::sleep(ms(300)).await;
        ctx.restarter().request("configuration changed");
        ctx.stopped().await;
        Ok(())
    });
    Ok(vec![service])
}

/// Rounds, with the last one repeating.
fn rounds(builds: &[Build]) {
    *ROUNDS.lock().unwrap() = builds.to_vec();
}

fn built() -> usize {
    *BUILT.lock().unwrap()
}

/// The arguments of a start below `root`, with `--set` for each pair.
fn args(root: &Path, sets: &[&str]) -> Vec<String> {
    let mut args = vec!["--root".to_string(), root.display().to_string()];
    for set in sets {
        args.extend(["--set".to_string(), (*set).to_string()]);
    }
    args
}

/// Waits until the host is in `status`.
fn reaches(host: &Host, status: Status) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while host.status() != status {
        assert!(
            Instant::now() < deadline,
            "not {status:?}: {:?}",
            host.status()
        );
        thread::sleep(ms(5));
    }
}

/// Starts the host on another thread; joining it gives what start returned.
fn start_aside(host: &'static Host, args: Vec<String>) -> thread::JoinHandle<Code> {
    thread::spawn(move || host.start(&args))
}

/// The threads of this process, where `/proc` shows them: on Linux and Android.
fn threads() -> Option<usize> {
    std::fs::read_dir("/proc/self/task")
        .ok()
        .map(Iterator::count)
}

/// Waits up to a second for this process to have `count` threads again, and returns how many
/// it has: a joined thread leaves `/proc` a moment after the join returns.
fn threads_back_to(count: Option<usize>) -> Option<usize> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let now = threads();
        if now == count || Instant::now() > deadline {
            return now;
        }
        thread::sleep(ms(10));
    }
}

/// Rivium's panic hook writes panics to the log once logging is installed: this test's own
/// failures go to standard error as well.
fn failures_to_stderr() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
}

/// The main log file.
fn log(root: &Path) -> String {
    std::fs::read_to_string(root.join("logs/probe/probe.log")).unwrap()
}

fn last_error(host: &Host) -> String {
    host.last_error().unwrap_or_default()
}

#[test]
fn the_embedded_host() {
    // The embedded host reads no environment: this would be an invalid configuration.
    #[expect(unsafe_code, reason = "setting a variable")]
    // SAFETY: no other thread of this test process runs yet.
    unsafe {
        std::env::set_var("PROBE_LIFECYCLE__STOP_TIMEOUT", "0s");
    }
    let root: PathBuf =
        std::env::temp_dir().join(format!("rivium-embedded-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let host: &'static Host = Box::leak(Box::new(Host::new::<Probe>()));
    let fast = ["lifecycle.startup_timeout=1s", "lifecycle.stop_timeout=1s"];
    rounds(&[serving]);

    // Nothing starts: the code says why, and the host stays idle.
    std::fs::write(root.join("file"), "").unwrap();
    let cases: [(Vec<String>, Code, &str); 4] = [
        (
            args(&root, &["log.file.dir=file/logs"]),
            Code::CantCreate,
            "CantCreate: cannot write the log files: ",
        ),
        (
            Vec::new(),
            Code::Usage,
            "Usage: the embedded host must pass --root",
        ),
        (
            vec!["--bogus".into()],
            Code::Usage,
            "Usage: unknown argument `--bogus`",
        ),
        (
            args(&root, &["lifecycle.stop_timeout=0s"]),
            Code::Config,
            "Config: invalid configuration: lifecycle.stop_timeout (host --set lifecycle.stop_timeout): must be between 1s and 2m",
        ),
    ];
    for (args, code, error) in cases {
        assert_eq!(host.start(&args), code, "{args:?}");
        assert!(last_error(host).starts_with(error), "{}", last_error(host));
        assert_eq!(host.status(), Status::Idle);
    }
    failures_to_stderr();
    assert_eq!(built(), 0);

    // Running: Ok, which clears the last error. Stopping: Ok, once the last lines are in the
    // file, though another thread keeps the log queue full meanwhile.
    assert_eq!(host.start(&args(&root, &[])), Code::Ok);
    assert_eq!((host.status(), host.last_error()), (Status::Running, None));
    assert_eq!(host.start(&args(&root, &[])), Code::AlreadyRunning);
    let busy = AtomicBool::new(true);
    let log = thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while busy.load(Ordering::Relaxed) {
                    tracing::info!("keeping the log queue full");
                }
            });
        }
        assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
        let log = log(&root);
        busy.store(false, Ordering::Relaxed);
        log
    });
    assert_eq!(host.status(), Status::Idle);
    let stopped = "rivium::embedded: stopped code=\"Ok\"";
    assert!(log.contains(stopped), "the last lines are in the file");
    assert_eq!(host.stop(Duration::from_secs(4)), Code::NotRunning);
    // The baseline: logging keeps two threads from the first start on.
    thread::sleep(ms(300));
    let baseline = threads();
    for _ in 0..3 {
        assert_eq!(host.start(&args(&root, &[])), Code::Ok);
        assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
        let left = threads_back_to(baseline);
        assert_eq!(left, baseline, "every thread of a start ends with its stop");
    }

    // Starting: another start is refused at once; a stop cancels the waiting start.
    rounds(&[slow_to_start]);
    let waiting = start_aside(host, args(&root, &[]));
    reaches(host, Status::Starting);
    assert_eq!(host.start(&args(&root, &[])), Code::AlreadyRunning);
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    assert_eq!(waiting.join().unwrap(), Code::Cancelled);
    assert_eq!(host.status(), Status::Idle);

    // The first round ends before it runs: StartupFailed, without another round.
    let never_ready: Build = || scripted(Frontline, vec![Step::UntilStopped]);
    let failing: Build = || scripted(Frontline, vec![Step::Fail("no device")]);
    let panicking: Build = || panic!("composition root");
    let erring: Build = || Error::e_explain(kinds::UNAVAILABLE, "no device");
    let restarting_early: Build = || {
        let service = rivium::service("config", Background, |ctx| async move {
            ctx.restarter().request("early");
            ctx.stopped().await;
            Ok(())
        });
        Ok(vec![service])
    };
    let cases: [(Build, &str); 5] = [
        (
            failing,
            "StartupFailed: startup failed: service http: no device",
        ),
        (
            panicking,
            "StartupFailed: startup failed: panic: composition root",
        ),
        (
            erring,
            "StartupFailed: startup failed: the services cannot be built: no device",
        ),
        (
            restarting_early,
            "StartupFailed: startup failed: restart requested: early",
        ),
        (
            never_ready,
            "StartupFailed: startup failed: not ready within 1s: http",
        ),
    ];
    for (build, error) in cases {
        rounds(&[build, serving]);
        let before = built();
        assert_eq!(
            host.start(&args(&root, &fast)),
            Code::StartupFailed,
            "{error}"
        );
        assert_eq!(last_error(host), error);
        assert_eq!((host.status(), built()), (Status::Idle, before + 1));
    }

    // No result within startup_timeout + stop_timeout + 2s: StartupFailed, then stopping until
    // the stuck composition root lets go.
    let stuck: Build = || {
        thread::sleep(Duration::from_secs(6));
        serving()
    };
    rounds(&[stuck]);
    let started = Instant::now();
    assert_eq!(host.start(&args(&root, &fast)), Code::StartupFailed);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(6),
        "{took:?}"
    );
    assert!(last_error(host).starts_with("StartupFailed: no result within"));
    assert_eq!(host.status(), Status::Stopping);
    assert_eq!(host.start(&args(&root, &fast)), Code::Busy);
    assert_eq!(host.stop(ms(100)), Code::StopTimedOut);
    assert_eq!(host.status(), Status::Stopping);
    assert_eq!(host.stop(Duration::from_secs(10)), Code::Ok);
    assert_eq!(host.status(), Status::Idle);

    // Stopping: a start is refused, and a second stop waits for the same stop.
    let slow_to_stop: Build =
        || scripted(Frontline, vec![Step::Ready, Step::IgnoreStop(ms(1_500))]);
    rounds(&[slow_to_stop]);
    let slow = ["lifecycle.stop_timeout=3s"];
    assert_eq!(host.start(&args(&root, &slow)), Code::Ok);
    let stopping = thread::spawn(|| host.stop(Duration::from_secs(5)));
    reaches(host, Status::Stopping);
    assert_eq!(host.start(&args(&root, &slow)), Code::Busy);
    assert_eq!(host.stop(Duration::from_secs(5)), Code::Ok);
    assert_eq!(stopping.join().unwrap(), Code::Ok);
    assert_eq!(host.status(), Status::Idle);

    // A service still running at the deadline is abandoned: StopTimedOut. A timeout under the
    // host's own second leaves the services no time at all.
    let stuck_stopping: Build = || {
        scripted(
            Frontline,
            vec![Step::Ready, Step::IgnoreStop(Duration::from_secs(30))],
        )
    };
    rounds(&[stuck_stopping]);
    for (timeout, within) in [(Duration::from_secs(5), 2_500), (ms(500), 500)] {
        assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
        let asked = Instant::now();
        assert_eq!(host.stop(timeout), Code::StopTimedOut);
        assert!(asked.elapsed() < ms(within), "{:?}", asked.elapsed());
        assert_eq!(
            last_error(host),
            "StopTimedOut: stop timed out\nabandoned: http"
        );
        assert_eq!(host.status(), Status::Idle);
    }

    // A service that fails while it stops: the stop still succeeded.
    let failing_stop: Build = || {
        scripted(
            Frontline,
            vec![Step::Ready, Step::UntilStopped, Step::Fail("flush failed")],
        )
    };
    rounds(&[failing_stop]);
    assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    assert_eq!(
        last_error(host),
        "Fault: stopped after a fault: service http: flush failed"
    );

    // A fault restarts in the process after a backoff; the last error stays for diagnosis.
    rounds(&[faulting, ready_soon]);
    assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
    assert_eq!(host.last_error(), None);
    reaches(host, Status::Restarting);
    assert_eq!(host.start(&args(&root, &fast)), Code::AlreadyRunning);
    reaches(host, Status::Starting);
    let fault = "Fault: stopped after a fault: service http: device gone";
    assert_eq!(last_error(host), fault);
    reaches(host, Status::Running);
    assert_eq!(last_error(host), fault);
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);

    // A stop while restarting cancels the rebuild.
    rounds(&[faulting, serving]);
    assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
    reaches(host, Status::Restarting);
    let before = built();
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    assert_eq!((host.status(), built()), (Status::Idle, before));

    // A restart request within 10s of the start counts as a failure: rebuilt after a backoff.
    rounds(&[restarting, serving]);
    assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
    reaches(host, Status::Restarting);
    reaches(host, Status::Running);
    assert_eq!(
        last_error(host),
        "Restart: restart requested within 10s of the last start: configuration changed"
    );
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);

    // Every service finishes on its own: the host stops by itself.
    let finishing: Build = || scripted(Background, vec![Step::Ready, Step::Return]);
    rounds(&[finishing]);
    assert_eq!(host.start(&args(&root, &fast)), Code::Ok);
    reaches(host, Status::Idle);
    assert_eq!(host.stop(Duration::from_secs(4)), Code::NotRunning);

    // Calls from many threads at once: one start wins; every stop either stops the services or
    // waits for that stop, or comes after it.
    rounds(&[serving]);
    let starts: Vec<_> = (0..8)
        .map(|_| start_aside(host, args(&root, &[])))
        .collect();
    let mut codes: Vec<Code> = starts
        .into_iter()
        .map(|start| start.join().unwrap())
        .collect();
    codes.sort_by_key(|code| code.ffi());
    assert_eq!(codes[0], Code::Ok);
    assert!(
        codes[1..].iter().all(|code| *code == Code::AlreadyRunning),
        "{codes:?}"
    );
    let stops: Vec<_> = (0..8)
        .map(|_| thread::spawn(|| host.stop(Duration::from_secs(4))))
        .collect();
    let codes: Vec<Code> = stops.into_iter().map(|stop| stop.join().unwrap()).collect();
    assert!(codes.contains(&Code::Ok), "{codes:?}");
    assert!(
        codes
            .iter()
            .all(|code| matches!(code, Code::Ok | Code::NotRunning))
    );
    assert_eq!(host.status(), Status::Idle);
    assert_eq!(
        threads_back_to(baseline),
        baseline,
        "no thread is left over"
    );

    let _ = std::fs::remove_dir_all(&root);
}
