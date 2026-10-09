//! The composition roots: an [`App`] per program, over the same business modules.

use rivium::{App, AppContext, Result, Service};
use serde::{Deserialize, Serialize};

use crate::collect::{self, CollectSettings};
use crate::meter::{self, Reading, SamplerSettings};
use crate::query::{self, QuerySettings};

/// The `station` program: samples the meter and answers queries for its reading.
pub struct Station;

impl App for Station {
    const NAME: &'static str = "station";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = StationConfig;

    fn services(config: &StationConfig, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let reading = Reading::default();
        Ok(vec![
            query::query(&config.query, &reading),
            meter::sampler(&config.sampler, &reading),
        ])
    }
}

/// The configuration of `station`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StationConfig {
    /// `[sampler]`
    pub sampler: SamplerSettings,
    /// `[query]`
    pub query: QuerySettings,
}

/// The `collector` program: asks stations for their readings.
pub struct Collector;

impl App for Collector {
    const NAME: &'static str = "collector";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = CollectorConfig;

    fn services(config: &CollectorConfig, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![collect::collect(&config.collect)])
    }
}

/// The configuration of `collector`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CollectorConfig {
    /// `[collect]`
    pub collect: CollectSettings,
}
