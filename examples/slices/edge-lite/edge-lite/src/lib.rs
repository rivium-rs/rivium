//! edge-lite: a validation slice of an edge collector. Units poll simulated devices, each under
//! the collector's own supervision: a unit that panics or fails is started again, and one that
//! keeps failing is given up. A store keeps the readings and writes the last of them out as the
//! service stops, within the stop deadline. An HTTP API answers with the status envelope: the
//! units, the configuration, which it can replace (checked, written, then a restart loads it),
//! and log export.
//!
//! ```toml
//! [http]
//! addr = "127.0.0.1:8080"
//!
//! [units]
//! count = 2
//! every = "1s"         # how often a unit polls its device
//! give_up_after = 5    # failures in a row after which a unit stays down
//!
//! [store]
//! every = "10s"        # how often the readings are written to data/readings.jsonl
//!
//! [faults]             # injected, for tests and drills
//! panic_every = 0      # a unit panics on every n-th poll of its run (0: never)
//! error_every = 0      # a unit fails on every n-th poll of its run (0: never)
//! flush_delay_ms = 0   # writing each reading takes this long
//! ```
//!
//! The unit events can go to a log file of their own:
//!
//! ```toml
//! [[log.files]]
//! name = "units"
//! filter = "edge_lite::units=info"
//! ```

use rivium::{App, AppContext, Result, Service};
use rivium_http::{HttpServer, HttpSettings};
use serde::{Deserialize, Serialize};

pub mod api;
pub mod store;
pub mod units;

pub use store::StoreSettings;
pub use units::{Faults, UnitSettings};

/// The program.
pub struct EdgeLite;

impl App for EdgeLite {
    const NAME: &'static str = "edge-lite";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let (readings, received) = tokio::sync::mpsc::channel(1024);
        let status = units::Status::new(config.units.count);
        let collector = units::collector(&config.units, &config.faults, &status, readings);
        let data = ctx.paths().data_dir();
        let store = store::store(&config.store, &config.faults, data, received);
        let router = api::router(config.clone(), status, ctx.clone());
        let http = HttpServer::new(&config.http, router, ctx);
        Ok(vec![Box::new(http), collector, store])
    }
}

/// The configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `[http]`
    pub http: HttpSettings,
    /// `[units]`
    pub units: UnitSettings,
    /// `[store]`
    pub store: StoreSettings,
    /// `[faults]`
    pub faults: Faults,
}
