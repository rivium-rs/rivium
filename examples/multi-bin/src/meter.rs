//! The meter: the latest reading, which the sampler updates and the other services report.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rivium::Service;
use serde::{Deserialize, Serialize};

/// The latest reading, shared between services.
#[derive(Clone, Debug, Default)]
pub struct Reading(Arc<AtomicU64>);

impl Reading {
    /// The value.
    #[must_use]
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Relaxed))
    }

    fn set(&self, value: f64) {
        self.0.store(value.to_bits(), Relaxed);
    }
}

/// `[sampler]`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SamplerSettings {
    /// How often the meter is read.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 3600>"
    )]
    pub every: Duration,
}

impl Default for SamplerSettings {
    fn default() -> Self {
        SamplerSettings {
            every: Duration::from_secs(1),
        }
    }
}

/// The sampler: reads the meter into `reading` every `settings.every`. The meter is simulated:
/// a slow sawtooth between 20.0 and 29.9.
#[must_use]
pub fn sampler(settings: &SamplerSettings, reading: &Reading) -> Box<dyn Service> {
    let reading = reading.clone();
    let mut samples = 0_u32;
    rivium::periodic("sampler", settings.every, move || {
        samples = (samples + 1) % 100;
        let value = 20.0 + f64::from(samples) / 10.0;
        reading.set(value);
        async move {
            tracing::debug!(value, "sampled");
            Ok(())
        }
    })
}
