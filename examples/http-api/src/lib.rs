//! An HTTP API on Rivium's process host: a small device registry that answers with the status
//! envelope ([`devices`]), the probes `/livez` and `/readyz`, and the binding of Rivium's log
//! export to the HTTP protocol of the service's clients ([`logs`]).
//!
//! ```toml
//! [http]
//! addr = "127.0.0.1:0"   # any free port; the `listening` event says which
//! request_timeout = "30s"
//! body_limit = "1MiB"
//! expose_internal_detail = false
//! probes = true
//! ```
//!
//! The access log can have a file of its own, which clients can then export as the kind of log
//! `access`:
//!
//! ```toml
//! [[log.files]]
//! name = "access"
//! filter = "rivium_http::access=info"
//! ```

use std::net::SocketAddr;

use rivium::{App, AppContext, Result, Service};
use rivium_http::{HttpServer, HttpSettings};
use serde::{Deserialize, Serialize};

pub mod devices;
pub mod logs;

/// The program.
pub struct Api;

impl App for Api {
    const NAME: &'static str = "http-api";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let router = devices::router().merge(logs::router(ctx.log_exporter().clone()));
        Ok(vec![Box::new(HttpServer::new(&config.http, router, ctx))])
    }
}

/// The configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// `[http]`
    pub http: HttpSettings,
}

impl Default for Config {
    fn default() -> Self {
        let mut http = HttpSettings::default();
        // Any free port: the tests run several instances at once.
        http.addr = SocketAddr::from(([127, 0, 0, 1], 0));
        Config { http }
    }
}
