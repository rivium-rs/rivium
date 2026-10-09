//! A sample service, to replace with the business: a periodic job in the background that logs
//! how long the service has been up.

use std::time::{Duration, Instant};

use rivium::Service;
use serde::{Deserialize, Serialize};

/// `[heartbeat]`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// How often.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 86400>"
    )]
    pub every: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            every: Duration::from_secs(60),
        }
    }
}

/// The service: a tick right away, then every `settings.every`.
pub(crate) fn service(settings: &Settings) -> Box<dyn Service> {
    let started = Instant::now();
    rivium::periodic("heartbeat", settings.every, move || {
        let up_secs = started.elapsed().as_secs();
        async move {
            tracing::info!(up_secs, "heartbeat");
            Ok(())
        }
    })
}
