//! The application: what a service program is made of, and what building its services gets.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::{Paths, Report};
use crate::lifecycle::Supervisor;
use crate::lifecycle::health::{HealthRegistry, Readiness};
use crate::lifecycle::service::{Restarter, Service};
use crate::{Result, log};

/// A service program: its name and version, its configuration, and its services. A host runs
/// it: [`process::run`](crate::process::run) as a program of its own. The host loads the
/// configuration, installs logging, builds the services and runs them, and builds them again
/// for a restart in the process.
///
/// ```no_run
/// use std::time::Duration;
///
/// #[derive(Default, serde::Serialize, serde::Deserialize)]
/// struct Config {
///     greeting: String,
/// }
///
/// struct Greeter;
///
/// impl rivium::App for Greeter {
///     const NAME: &'static str = "greeter";
///     const VERSION: &'static str = env!("CARGO_PKG_VERSION");
///     type Config = Config;
///
///     fn services(config: &Config, _: &rivium::AppContext) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
///         let greeting = config.greeting.clone();
///         Ok(vec![rivium::periodic("greet", Duration::from_secs(60), move || {
///             tracing::info!(greeting, "hello");
///             async { Ok(()) }
///         })])
///     }
/// }
///
/// fn main() -> std::process::ExitCode {
///     rivium::process::run::<Greeter>()
/// }
/// ```
pub trait App: 'static {
    /// The name, in kebab-case: in logs and file names, and in upper snake case as the prefix of
    /// environment variables (`snmp-agent` reads `SNMP_AGENT_LOG__FILTER`).
    const NAME: &'static str;
    /// The version, as `--version` shows it: usually `env!("CARGO_PKG_VERSION")`.
    const VERSION: &'static str;
    /// The configuration file, relative to the root directory.
    const CONFIG_FILE: &'static str = "config.toml";
    /// The service's own configuration: every top-level section but Rivium's `log` and
    /// `lifecycle`.
    type Config: Serialize + DeserializeOwned + Default + Send + Sync + 'static;

    /// Builds the services, without starting them: nothing here waits or spawns. Work that
    /// waits, such as connecting, belongs in each service's `run`, before it is ready.
    ///
    /// # Errors
    ///
    /// When the services cannot be built, which fails the startup.
    fn services(config: &Self::Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>>;

    /// Layers to add to logging, such as a bridge to another telemetry system. Called once,
    /// when logging is first installed in the process.
    fn log_layers(_config: &Self::Config) -> Vec<log::BoxedLayer> {
        Vec::new()
    }
}

/// What building the services gets from the host: where the files are, who the service is,
/// the health items and readiness of the run, restart requests, and the check for a new
/// configuration.
pub struct AppContext {
    paths: Paths,
    identity: Identity,
    health: HealthRegistry,
    readiness: Readiness,
    restarter: Restarter,
    check: Check,
    #[cfg(feature = "log-export")]
    exporter: log::LogExporter,
}

/// Checks a candidate configuration file.
pub(crate) type Check = Box<dyn Fn(&str) -> std::result::Result<(), Report> + Send + Sync>;

impl AppContext {
    /// The context of the run `supervisor` is about to supervise.
    pub(crate) fn new(
        paths: Paths,
        identity: Identity,
        supervisor: &Supervisor,
        check: Check,
    ) -> Self {
        AppContext {
            paths,
            identity,
            health: supervisor.health(),
            readiness: supervisor.readiness(),
            restarter: supervisor.restarter(),
            check,
            #[cfg(feature = "log-export")]
            exporter: log::exporter(),
        }
    }

    /// Where the service's files are; [`Paths::resolve`] turns a path from the configuration
    /// into one below the root.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Who the service is.
    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Where services register the health items that readiness depends on.
    #[must_use]
    pub fn health(&self) -> &HealthRegistry {
        &self.health
    }

    /// The readiness of the run, for probes.
    #[must_use]
    pub fn readiness(&self) -> &Readiness {
        &self.readiness
    }

    /// Asks for a restart of the run, for example once a new configuration is written.
    #[must_use]
    pub fn restarter(&self) -> &Restarter {
        &self.restarter
    }

    /// Checks a candidate for the configuration file, as the host would load it at the next
    /// start: the same environment and `--set` overrides, with `candidate` as the file. A
    /// host that restarts in the process cannot change logging but for its filters, so then
    /// any other change to `[log]` is a problem too.
    ///
    /// # Errors
    ///
    /// Every problem with the candidate.
    pub fn check_config(&self, candidate: &str) -> std::result::Result<(), Report> {
        (self.check)(candidate)
    }

    /// Packs log files into an archive to download (feature `log-export`). It belongs to the
    /// process's logging, so an export outlives a restart in the process.
    #[cfg(feature = "log-export")]
    #[must_use]
    pub fn log_exporter(&self) -> &log::LogExporter {
        &self.exporter
    }
}

impl fmt::Debug for AppContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppContext")
            .field("paths", &self.paths)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Who a service is, for logs and telemetry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Identity {
    /// [`App::NAME`].
    pub name: &'static str,
    /// [`App::VERSION`].
    pub version: &'static str,
    /// `lifecycle.instance`: which of several instances of the service this is; empty when
    /// there is one.
    pub instance: String,
}
