//! Minimal non-HTTP application shared by private host tests.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// The configuration of the private application.
#[derive(Default, Serialize, Deserialize)]
pub struct Config {}

/// A service with no external deployment prerequisites.
pub struct Application;

impl rivium::App for Application {
    const NAME: &'static str = "test-process";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(
        _: &Config,
        _: &rivium::AppContext,
    ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
        Ok(vec![rivium::service(
            "worker",
            rivium::ServiceKind::Background,
            |ctx| async move {
                ctx.ready();
                ctx.stopped().await;
                Ok(())
            },
        )])
    }
}
