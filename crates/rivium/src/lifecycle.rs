//! The service lifecycle.

use std::time::Duration;

use serde::{Deserialize, Serialize};

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
