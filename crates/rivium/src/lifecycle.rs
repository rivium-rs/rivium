//! The service lifecycle: the supervisor that starts, watches and stops a run of services.
//!
//! A run starts every service, enters [`Phase::Running`](crate::Phase) once each one has called
//! `ready()`, and stops when the host asks it to, when a service asks for a restart, or when a
//! service fails. Stopping has one deadline, `stop_timeout` from the moment the run starts to
//! stop (earlier when the host's stop request brings a shorter budget): frontline services are
//! asked to stop first, background services once no frontline service runs, and whatever still
//! runs at the deadline is abandoned. The first reason to stop decides the [`Outcome`]; a
//! failure while stopping turns a stop or restart into a fault.
//!
//! The hosts run the supervisor; services never need it. Tests use it to drive services:
//!
//! ```
//! use std::time::Duration;
//!
//! use rivium::lifecycle::{Outcome, StopReason, Supervisor, stop_channel};
//! use rivium::ServiceKind;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let echo = rivium::service("echo", ServiceKind::Frontline, |ctx| async move {
//!     ctx.ready();
//!     ctx.stopped().await;
//!     Ok(())
//! });
//! let supervisor = Supervisor::new(Duration::from_secs(30), Duration::from_secs(3)).with(echo);
//! let (stop, stops) = stop_channel();
//! stop.stop(StopReason::Signal { name: "SIGTERM", number: 15 });
//! assert!(matches!(supervisor.run(stops).await, Outcome::Stopped));
//! # });
//! ```
//!
//! Failures are logged once, where the supervisor learns of them: the one that stops the run as
//! an error, failures of other services while it stops as warnings, one line per service. Their
//! kinds are `ServiceExited` (a service returned before it was ready, or a frontline service
//! before it was asked to stop), `ServicePanicked`, `StartupTimedOut` and `InvalidServices` (no
//! services, or two with one name); other failures keep the kind the service gave them.

pub(crate) mod health;
mod machine;
pub(crate) mod service;
mod supervisor;
pub(crate) mod tasks;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use supervisor::{Outcome, StopHandle, StopReason, StopReceiver, Supervisor, stop_channel};

use crate::config::de::{duration, serialize_duration};

/// `[lifecycle]`: the budgets of a run and what a restart request does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LifecycleSettings {
    /// How long every service may take to become ready.
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "duration::<_, 1, 600>"
    )]
    pub(crate) startup_timeout: Duration,
    /// How long services and their tasks may take to stop, from the stop request on.
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "duration::<_, 1, 120>"
    )]
    pub(crate) stop_timeout: Duration,
    /// What a restart request does in the process host; the embedded host always restarts in
    /// process.
    pub(crate) restart: Restart,
    /// The instance of the service, for logs; empty when there is only one.
    pub(crate) instance: String,
}

/// How the process host restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Restart {
    /// Exit with code 75 and let the supervisor (systemd, launchd, a Windows service wrapper)
    /// start the process again.
    Exit,
    /// Load the configuration again and rebuild the services in the same process, for
    /// deployments whose supervisor does not restart.
    InProcess,
}

impl Default for LifecycleSettings {
    fn default() -> Self {
        LifecycleSettings {
            startup_timeout: Duration::from_secs(30),
            stop_timeout: Duration::from_secs(3),
            restart: Restart::Exit,
            instance: String::new(),
        }
    }
}
