//! The startup loop on paused time: what ends it, the backoff, giving up, clearing the count,
//! restart requests that come too soon, stops during and between rounds, and panics; and the
//! check of a candidate configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rivium_error::{Error, kinds};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{End, Failure, Overrides, Round, check, run};
use crate::lifecycle::{StopReason, Supervisor};
use crate::log::LogSettings;
use crate::{Code, ServiceKind, service};

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// What one round does.
#[derive(Clone, Copy, Debug)]
enum Script {
    /// Serves until it is stopped.
    Serve,
    /// A ready service fails after this many seconds: a fault.
    FailAfter(u64),
    /// A ready service asks for a restart after this many seconds.
    RestartAfter(u64),
    /// The only service finishes: the run is over.
    Finish,
    /// Building the round panics, as a composition root can.
    Panic,
    /// Building the round fails with this code, as loading the configuration again can.
    BuildFails(Code),
}

/// The rounds of a test: one script per round, the last one repeating; records when each
/// round was built.
#[derive(Clone, Default)]
struct Rounds {
    built: Arc<Mutex<Vec<Instant>>>,
}

impl Rounds {
    fn build(
        &self,
        scripts: &[Script],
        in_process: bool,
    ) -> impl FnMut(bool) -> Result<Round, Failure> {
        let (built, scripts) = (Arc::clone(&self.built), scripts.to_vec());
        move |_first| {
            let mut built = built.lock().unwrap();
            let script = scripts[built.len().min(scripts.len() - 1)];
            built.push(Instant::now());
            drop(built);
            let service = match script {
                Script::Panic => panic!("composition root"),
                Script::BuildFails(code) => return Err(Failure::new(code, "cannot build")),
                Script::Serve => service("http", ServiceKind::Frontline, |ctx| async move {
                    ctx.ready();
                    ctx.stopped().await;
                    Ok(())
                }),
                Script::FailAfter(after) => {
                    service("http", ServiceKind::Frontline, move |ctx| async move {
                        ctx.ready();
                        tokio::time::sleep(secs(after)).await;
                        Error::e_explain(kinds::UNAVAILABLE, "device gone")
                    })
                }
                Script::RestartAfter(after) => {
                    service("config", ServiceKind::Background, move |ctx| async move {
                        ctx.ready();
                        tokio::time::sleep(secs(after)).await;
                        ctx.restarter().request("configuration changed");
                        ctx.stopped().await;
                        Ok(())
                    })
                }
                Script::Finish => service("job", ServiceKind::Background, |ctx| async move {
                    ctx.ready();
                    Ok(())
                }),
            };
            let supervisor = Supervisor::new(secs(30), secs(3)).with(service);
            Ok(Round {
                supervisor,
                in_process,
            })
        }
    }

    /// The time between one build and the next, in seconds.
    fn gaps(&self) -> Vec<u64> {
        let built = self.built.lock().unwrap();
        (built.windows(2))
            .map(|pair| (pair[1] - pair[0]).as_secs())
            .collect()
    }
}

/// Stop requests sent at these times in milliseconds, SIGTERM first, then SIGINT.
fn stops_at(times: &[u64]) -> mpsc::UnboundedReceiver<StopReason> {
    let (stop, stops) = mpsc::unbounded_channel();
    let start = Instant::now();
    let reasons = [("SIGTERM", 15), ("SIGINT", 2)];
    let times: Vec<_> = times
        .iter()
        .map(|at| start + Duration::from_millis(*at))
        .collect();
    tokio::spawn(async move {
        for (at, (name, number)) in times.into_iter().zip(reasons) {
            tokio::time::sleep_until(at).await;
            stop.send(StopReason::Signal { name, number }).unwrap();
        }
        // Hold the sender, as the hosts do.
        std::future::pending::<()>().await;
    });
    stops
}

async fn ends(scripts: &[Script], in_process: bool, stops: &[u64]) -> (End, Rounds) {
    let rounds = Rounds::default();
    let end = run(
        rounds.build(scripts, in_process),
        in_process,
        &mut stops_at(stops),
    )
    .await;
    (end, rounds)
}

fn shown(end: &End) -> (Code, String) {
    (end.code, end.message.clone().unwrap_or_default())
}

#[tokio::test(start_paused = true)]
async fn without_in_process_restarts_the_first_round_ends_the_loop() {
    let cases = [
        (Script::Serve, Code::Ok, ""),
        (Script::Finish, Code::Ok, ""),
        (
            Script::FailAfter(1),
            Code::Fault,
            "stopped after a fault: service http: device gone",
        ),
        (
            Script::RestartAfter(1),
            Code::Restart,
            "restart requested: configuration changed",
        ),
        (
            Script::Panic,
            Code::StartupFailed,
            "startup failed: panic: composition root",
        ),
        (
            Script::BuildFails(Code::StartupFailed),
            Code::StartupFailed,
            "cannot build",
        ),
    ];
    for (script, code, message) in cases {
        let (end, rounds) = ends(&[script], false, &[5_000]).await;
        assert_eq!(shown(&end), (code, message.to_string()), "{script:?}");
        assert_eq!(rounds.built.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn in_process_failures_back_off_and_the_tenth_in_a_row_gives_up() {
    let start = Instant::now();
    let (end, rounds) = ends(&[Script::FailAfter(0)], true, &[]).await;
    assert_eq!(rounds.gaps(), [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    assert_eq!(start.elapsed(), secs(243), "about four minutes");
    let (code, message) = shown(&end);
    assert_eq!(code, Code::Fault);
    assert_eq!(
        message,
        "gave up after 10 failures in a row; the last: stopped after a fault: service http: device gone"
    );
}

#[tokio::test(start_paused = true)]
async fn a_round_running_for_ten_minutes_clears_the_count() {
    let mut scripts = vec![Script::FailAfter(0); 5];
    scripts.push(Script::FailAfter(600));
    scripts.push(Script::FailAfter(0));
    let (end, rounds) = ends(&scripts, true, &[]).await;
    let gaps = rounds.gaps();
    // Five failures, then a round that ran ten minutes: its failure is the first again.
    assert_eq!(gaps[..6], [1, 2, 4, 8, 16, 600 + 1]);
    assert_eq!(gaps[6..], [2, 4, 8, 16, 32, 60, 60, 60]);
    assert_eq!(end.code, Code::Fault);
}

#[tokio::test(start_paused = true)]
async fn a_restart_request_rebuilds_at_once_unless_it_comes_too_soon() {
    let scripts = [
        Script::RestartAfter(10),
        Script::RestartAfter(1),
        Script::Serve,
    ];
    let (end, rounds) = ends(&scripts, true, &[100_000]).await;
    // The first request came 10s after the build: at once. The second came 1s after its
    // build: a failure, so a backoff of 1s.
    assert_eq!(rounds.gaps(), [10, 1 + 1]);
    assert_eq!(shown(&end), (Code::Ok, String::new()));
}

#[tokio::test(start_paused = true)]
async fn a_stop_request_ends_the_loop_after_its_round() {
    let start = Instant::now();
    let (end, rounds) = ends(&[Script::Serve], true, &[5_000]).await;
    assert_eq!(shown(&end), (Code::Ok, String::new()));
    assert_eq!((start.elapsed(), rounds.gaps().len()), (secs(5), 0));

    // The round fails while it stops: the loop still ends, with the fault.
    let failing = service("http", ServiceKind::Frontline, |ctx| async move {
        ctx.ready();
        ctx.stopped().await;
        Error::e_explain(kinds::UNAVAILABLE, "flush failed")
    });
    let mut once = Some(Supervisor::new(secs(30), secs(3)).with(failing));
    let build = move |_| {
        let supervisor = once.take().expect("one round only");
        Ok(Round {
            supervisor,
            in_process: true,
        })
    };
    let end = run(build, true, &mut stops_at(&[1_000])).await;
    assert_eq!(end.code, Code::Fault);
}

#[tokio::test(start_paused = true)]
async fn a_stop_request_between_rounds_ends_the_loop_at_once() {
    let start = Instant::now();
    // Stopped during the first backoff.
    let (end, rounds) = ends(&[Script::FailAfter(0)], true, &[500]).await;
    let half = Duration::from_millis(500);
    assert_eq!(
        (shown(&end), start.elapsed()),
        ((Code::Ok, String::new()), half)
    );
    assert_eq!(rounds.built.lock().unwrap().len(), 1);
    // Two stop requests between rounds: the second aborts.
    let (end, _) = ends(&[Script::FailAfter(0)], true, &[500, 500]).await;
    assert_eq!(end.code, Code::Aborted(2));
}

#[tokio::test(start_paused = true)]
async fn panics_and_failed_rebuilds_count_as_failures_in_process() {
    let scripts = [
        Script::Panic,
        Script::FailAfter(0),
        Script::BuildFails(Code::Config),
    ];
    let (end, rounds) = ends(&scripts, true, &[]).await;
    assert_eq!(rounds.gaps(), [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    // The last failure gives its code: a configuration that cannot be loaded again.
    assert_eq!(
        shown(&end),
        (
            Code::Config,
            "gave up after 10 failures in a row; the last: cannot build".to_string()
        )
    );
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    port: u16,
}

#[test]
fn a_candidate_configuration_is_checked_as_the_next_start_would_load_it() {
    let overrides = Overrides {
        env: None,
        sets: vec![("log.filter".to_string(), "info".to_string())],
        host: true,
    };
    let file = PathBuf::from("/srv/app/config.toml");
    let exiting = check::<Config>("app", file.clone(), overrides.clone(), None);
    assert!(exiting("port = 1\n[log.file]\ndir = \"elsewhere\"\n").is_ok());
    let problems = exiting("port = \"x\"\n[lifecycle]\nstop_timeout = \"0s\"\n").unwrap_err();
    assert_eq!(
        problems.to_string(),
        "invalid configuration: lifecycle.stop_timeout (file /srv/app/config.toml): must be between 1s and 2m, got \"0s\"\n\
         invalid configuration: port (file /srv/app/config.toml): invalid type: string \"x\", expected u16"
    );
    // A host that restarts in the process keeps its logging: only filters may change.
    let installed = LogSettings::new("app");
    let staying = check::<Config>("app", file, overrides, Some(installed));
    assert!(staying("[log]\nfilter = \"debug\"\n[log.console]\nfilter = \"warn\"\n").is_ok());
    let problems =
        staying("[log.file]\ndir = \"elsewhere\"\nmax_file_size = \"32MiB\"\n").unwrap_err();
    assert_eq!(
        problems.to_string(),
        "invalid configuration: log.file.dir (file /srv/app/config.toml): cannot change while the process runs: restart the process instead\n\
         invalid configuration: log.file.max_file_size (file /srv/app/config.toml): cannot change while the process runs: restart the process instead"
    );
}
