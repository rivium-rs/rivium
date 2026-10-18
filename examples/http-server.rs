//! An HTTP service with readiness probes and graceful shutdown.

use std::net::SocketAddr;

use axum::{Router, routing::get};
use rivium::{Result, Service};
use rivium_host::{App, Code, Context};
use rivium_http::{ApiResponse, HttpServer};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([127, 0, 0, 1], 8080).into(),
        }
    }
}

#[derive(Serialize)]
struct Greeting {
    message: &'static str,
}

struct HttpApp;

impl App for HttpApp {
    const NAME: &'static str = "http-server";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, _: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        let router = Router::new().route(
            "/",
            get(|| async {
                ApiResponse::Data(Greeting {
                    message: "Hello from Rivium",
                })
            }),
        );
        Ok(vec![Box::new(HttpServer::new(
            "http",
            config.listen,
            router,
        ))])
    }
}

fn main() -> Code {
    rivium_host::process::run::<HttpApp>()
}
