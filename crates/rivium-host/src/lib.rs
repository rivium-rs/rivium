//! Explicit process and embedded ownership for a long-running application.
#[cfg(feature = "embedded")]
pub mod embedded;
mod options;
#[cfg(feature = "process")]
pub mod process;
#[cfg_attr(not(any(feature = "process", feature = "embedded")), allow(dead_code))]
mod reload;
#[cfg_attr(not(any(feature = "process", feature = "embedded")), allow(dead_code))]
mod session;

pub use options::{Action, Options, Paths};
use rivium::{Error, HealthRegistry, Readiness, Result, RunReport, Service};
pub use rivium_util::{config, log};
use serde::{Serialize, de::DeserializeOwned};
pub use session::{Lifecycle, Restart};
use std::{sync::Arc, time::Duration};

/// Application composition; hooks must not install process-global state.
pub trait App: Send + Sync + 'static {
    const NAME: &'static str;
    const VERSION: &'static str;
    type Config: Default + Serialize + DeserializeOwned + Send + Sync + 'static;
    /// Construct services without starting their work.
    fn services(
        config: &Self::Config,
        context: &Context<Self::Config>,
    ) -> Result<Vec<Box<dyn Service>>>;
    fn validate(_config: &Self::Config) -> Result<()> {
        Ok(())
    }
    /// Purely select live fields; `None` keeps all application fields cold.
    fn reload_config(
        _current: &Self::Config,
        _candidate: &Self::Config,
    ) -> Result<Option<Self::Config>> {
        Ok(None)
    }
    fn log_layers() -> Vec<log::ExtraLayer> {
        Vec::new()
    }
}

/// Generation-bound control that closes configuration admission before stopping.
#[derive(Clone)]
pub struct HostControl {
    pub(crate) inner: rivium::Control,
    pub(crate) reload: reload::Requester,
}
impl HostControl {
    /// Close publication admission and request this generation's stop.
    pub fn stop(&self, reason: rivium::StopReason) {
        self.reload.close();
        self.inner.stop(reason);
    }
    /// Close publication admission and request a replacement generation.
    pub fn restart(&self, reason: impl Into<String>) {
        self.reload.close();
        self.inner.restart(reason);
    }
}

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

type ConfigCheck = dyn Fn(&str) -> Result<()> + Send + Sync;

/// Read-only configuration and control bound to one generation.
pub struct Context<C> {
    pub(crate) config: config::Handle<C>,
    pub(crate) reload: reload::Requester,
    pub(crate) paths: Paths,
    pub(crate) readiness: Readiness,
    pub(crate) control: HostControl,
    pub(crate) health: HealthRegistry,
    pub(crate) check: Arc<ConfigCheck>,
    #[cfg(feature = "log-export")]
    pub(crate) exporter: log::Exporter,
}
impl<C> Clone for Context<C> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            reload: self.reload.clone(),
            paths: self.paths.clone(),
            readiness: self.readiness.clone(),
            control: self.control.clone(),
            health: self.health.clone(),
            check: self.check.clone(),
            #[cfg(feature = "log-export")]
            exporter: self.exporter.clone(),
        }
    }
}
impl<C> Context<C> {
    pub fn config(&self) -> config::Handle<C> {
        self.config.clone()
    }
    pub async fn reload(&self) -> Result<config::ReloadReport> {
        self.reload.request().await
    }
    pub fn paths(&self) -> &Paths {
        &self.paths
    }
    pub fn readiness(&self) -> Readiness {
        self.readiness.clone()
    }
    pub fn health(&self) -> &HealthRegistry {
        &self.health
    }
    pub fn control(&self) -> &HostControl {
        &self.control
    }
    pub fn check_config(&self, text: &str) -> Result<()> {
        (self.check)(text)
    }
    /// Validate and save without publishing or restarting. The source chain
    /// retains `AtomicWriteError`, including its committed/durability status.
    pub fn write_config(&self, text: &str) -> Result<()> {
        self.check_config(text)?;
        rivium_util::fs::atomic_write(self.paths.config_file(), text.as_bytes(), true)
            .map_err(|error| Error::internal(error.to_string()).with_source(error))
    }
    #[cfg(feature = "log-export")]
    pub fn log_exporter(&self) -> log::Exporter {
        self.exporter.clone()
    }
}

/// Stable process and FFI result vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Code {
    Ok,
    AlreadyRunning,
    NotRunning,
    Busy,
    Cancelled,
    Usage,
    StartupFailed,
    Fault,
    OsError,
    CantCreate,
    Restart,
    Config,
    StopTimedOut,
    Aborted(i32),
    Panicked,
}
impl Code {
    pub const fn exit(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Usage => 64,
            Self::StartupFailed => 69,
            Self::Fault | Self::Panicked => 70,
            Self::OsError => 71,
            Self::CantCreate => 73,
            Self::Restart => 75,
            Self::Config => 78,
            Self::StopTimedOut => 124,
            Self::Aborted(signal) => (128 + signal) as u8,
            _ => 70,
        }
    }
    pub const fn ffi(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::AlreadyRunning => 1,
            Self::NotRunning => 2,
            Self::Busy => 3,
            Self::Cancelled => 4,
            Self::Panicked => -99,
            code => -(code.exit() as i32),
        }
    }
}
impl std::process::Termination for Code {
    fn report(self) -> std::process::ExitCode {
        self.exit().into()
    }
}

/// Owners that still hold resources after closure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingResources {
    pub service_tasks: usize,
    pub reload_preparing: usize,
    pub log_workers: usize,
}
/// At most one retained failure per cleanup stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupStage {
    Services,
    Reload,
    Logger,
    Runtime,
}
#[derive(Clone, Debug)]
pub struct CleanupFailure {
    pub stage: CleanupStage,
    pub error: Error,
}
/// Closure, remaining ownership and errors are independent facts.
#[derive(Clone, Debug)]
pub struct CleanupReport {
    pub closed: bool,
    pub pending: PendingResources,
    pub failures: Vec<CleanupFailure>,
}
impl CleanupReport {
    pub fn is_drained(&self) -> bool {
        self.closed && self.pending == PendingResources::default()
    }
    pub fn is_success(&self) -> bool {
        self.is_drained() && self.failures.is_empty()
    }
}
impl Default for CleanupReport {
    fn default() -> Self {
        Self {
            closed: true,
            pending: PendingResources::default(),
            failures: Vec::new(),
        }
    }
}
/// Preserves the primary cause alongside service and cleanup evidence.
#[derive(Clone, Debug)]
pub struct HostReport {
    pub code: Code,
    pub primary: Option<Error>,
    pub run: Option<RunReport>,
    pub cleanup: CleanupReport,
}

#[cfg_attr(not(any(feature = "process", feature = "embedded")), allow(dead_code))]
const CLEANUP: Duration = Duration::from_secs(1);
/// Process-lifetime counters; individual logger counters are available separately.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub log_bytes: u64,
    pub log_failures: u64,
    pub log_deleted: u64,
    pub restarts: u64,
    pub faults: u64,
}
pub fn stats() -> Stats {
    let [log_bytes, log_failures, log_deleted] = log::counters();
    Stats {
        log_bytes,
        log_failures,
        log_deleted,
        restarts: session::RESTARTS.load(std::sync::atomic::Ordering::Relaxed),
        faults: session::FAULTS.load(std::sync::atomic::Ordering::Relaxed),
    }
}
