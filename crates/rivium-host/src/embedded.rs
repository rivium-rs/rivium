//! Restartable hosts that do not install process signals or read environment.

pub use crate::session::Status;
use crate::{
    Action, App, CleanupReport, Code, HostReport, Options, config,
    session::{self, Run},
};
use rivium::{Error, Result, StopReason, error::kinds};
use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Runner = fn(Options, Arc<Run>, bool);

/// A type-erased, thread-safe controller. Each start captures its own run record.
pub struct Host {
    name: &'static str,
    runner: Runner,
    current: Mutex<Option<Arc<Run>>>,
}

impl Host {
    pub fn new<A: App>() -> Self {
        Self {
            name: A::NAME,
            runner: session::run::<A>,
            current: Mutex::new(None),
        }
    }

    fn current(&self) -> Option<Arc<Run>> {
        crate::lock(&self.current).clone()
    }

    pub fn status(&self) -> Status {
        self.current()
            .map_or(Status::Idle, |run| crate::lock(&run.state).status)
    }

    /// A safe classification summary; configuration text and sources stay private.
    pub fn last_error(&self) -> Option<String> {
        self.last_report()
            .filter(|report| report.code != Code::Ok)
            .map(|report| {
                report.primary.map_or_else(
                    || format!("{:?}", report.code),
                    |error| format!("{:?}: {}", report.code, error.kind().name()),
                )
            })
    }

    pub fn last_report(&self) -> Option<HostReport> {
        self.current().and_then(|run| run.report())
    }

    pub fn active_tasks(&self) -> usize {
        self.current()
            .map_or(0, |run| run.cleanup().pending.service_tasks)
    }

    /// Wait for this captured run to close and release resources; never sends stop.
    pub async fn drain(&self, within: Duration) -> CleanupReport {
        let Some(run) = self.current() else {
            return CleanupReport::default();
        };
        let end = tokio::time::Instant::now()
            .checked_add(within)
            .unwrap_or_else(tokio::time::Instant::now);
        let generation = run.generation();
        loop {
            let notified = generation.completed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let report = generation.cleanup();
            if report.is_drained() || tokio::time::Instant::now() >= end {
                return report;
            }

            let poll_at = if report.closed {
                end.min(tokio::time::Instant::now() + Duration::from_millis(5))
            } else {
                end
            };
            tokio::select! {
                _ = notified => {},
                _ = tokio::time::sleep_until(poll_at) => {}
            }
        }
    }

    fn rejected_start(&self, error: Error) -> Code {
        let mut current = crate::lock(&self.current);
        if let Some(run) = current.as_ref() {
            let finished = crate::lock(&run.state).finished;
            if !finished || !run.cleanup().is_drained() {
                return Code::Usage;
            }
        }

        let run = Arc::new(Run::new());
        run.finish(Code::Usage, Some(error), None);
        let old = current.replace(run);
        drop(current);
        drop(old);
        Code::Usage
    }

    /// Start and wait for initial readiness or a reported startup failure.
    pub fn start(&self, args: &[String]) -> Code {
        let options = match Options::parse(self.name, args.iter().map(OsString::from), false) {
            Ok(options) => options,
            Err(error) => return self.rejected_start(error),
        };
        if options.action != Action::Run {
            return self
                .rejected_start(Error::invalid("embedded host accepts only the run command"));
        }

        let run = Arc::new(Run::new());
        {
            let mut current = crate::lock(&self.current);
            if let Some(previous) = current.as_ref() {
                let state = crate::lock(&previous.state);
                if !state.finished {
                    return if matches!(state.status, Status::Running | Status::Starting) {
                        Code::AlreadyRunning
                    } else {
                        Code::Busy
                    };
                }
                drop(state);
                if !previous.cleanup().is_drained() {
                    return Code::Busy;
                }
            }

            let old = current.replace(run.clone());
            drop(current);
            drop(old);
        }

        let task = run.clone();
        let runner = self.runner;
        if let Err(error) = std::thread::Builder::new()
            .name(format!("{}-host", self.name))
            .spawn(move || runner(options, task, true))
        {
            run.finish(Code::OsError, Some(error.into()), None);
            return Code::OsError;
        }

        let mut state = crate::lock(&run.state);
        while state.first.is_none() {
            state = run.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.first.unwrap()
    }

    pub async fn reload(&self) -> Result<config::ReloadReport> {
        let run = self
            .current()
            .ok_or_else(|| Error::new(kinds::UNAVAILABLE, "host has not started"))?;
        let generation = run.generation();
        let reload = generation
            .reload()
            .ok_or_else(|| Error::new(kinds::UNAVAILABLE, "host configuration is initializing"))?;
        reload.request().await
    }

    /// Request stop and spend at most `within` waiting for its captured run.
    /// A zero budget sends the request and returns immediately.
    pub fn stop(&self, within: Duration) -> Code {
        let Some(run) = self.current() else {
            return Code::NotRunning;
        };
        let end = Instant::now()
            .checked_add(within)
            .unwrap_or_else(Instant::now);
        {
            let state = crate::lock(&run.state);
            if state.finished {
                return Code::NotRunning;
            }
        }
        run.stop(StopReason::Deadline(within), Some(within));
        let mut state = crate::lock(&run.state);
        loop {
            if state.finished {
                return state
                    .report
                    .as_ref()
                    .map_or(Code::Fault, |report| report.code);
            }

            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Code::StopTimedOut;
            }
            state = run
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(run) = self.current() {
            run.stop(StopReason::Deadline(Duration::ZERO), Some(Duration::ZERO));
        }
    }
}
