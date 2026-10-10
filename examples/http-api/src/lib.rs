//! An HTTP API on Rivium's process host: a small device registry that answers with the status
//! envelope ([`devices`]), the probes `/livez` and `/readyz`, and the binding of Rivium's log
//! export to the HTTP protocol of the service's clients ([`logs`]). A layer outside the contract
//! layer takes the request id from a W3C `traceparent` header ([`trace_id`]).
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

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use rivium::{App, AppContext, Result, Service};
use rivium_http::{HttpServer, HttpSettings, RequestId};
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
        let server = HttpServer::new(&config.http, router, ctx);
        Ok(vec![Box::new(server.layer(middleware::from_fn(trace_id)))])
    }
}

/// Makes the trace id of a W3C `traceparent` header (`00-<trace id>-<parent id>-<flags>`) the
/// request id: the contract layer keeps an id that a layer outside it set, returns it in
/// `x-request-id` and logs every line of the request with it. Without the header, the contract
/// layer's own rules apply.
pub async fn trace_id(mut request: Request, next: Next) -> Response {
    let header = request.headers().get("traceparent");
    let traceparent = header
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let trace = traceparent.split('-').nth(1);
    let valid = |id: &&str| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(id) = trace.filter(valid).map(RequestId::new) {
        request.extensions_mut().insert(id);
    }
    next.run(request).await
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
