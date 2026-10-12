//! A private, non-HTTP application for testing host boundaries, not a teaching example.
#![forbid(unsafe_code)]

use rivium::{App, AppContext, Service, ServiceKind};
use serde::{Deserialize, Serialize};

/// The minimal application shared by host subjects.
pub struct VerifyApp;

/// Business configuration; host-owned settings remain outside this type.
#[derive(Default, Deserialize, Serialize)]
pub struct Config {
    /// Deterministic verification inputs.
    pub verify: Verify,
}

/// Only the case selection belongs to this verification application.
#[derive(Deserialize, Serialize)]
pub struct Verify {
    /// The selected boundary case.
    pub case: String,
}

impl Default for Verify {
    fn default() -> Self {
        Self {
            case: "idle".into(),
        }
    }
}

impl App for VerifyApp {
    const NAME: &'static str = "rivium-verify";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(_: &Config, _: &AppContext) -> rivium::Result<Vec<Box<dyn Service>>> {
        Ok(vec![rivium::service(
            "control",
            ServiceKind::Background,
            |ctx| async move {
                ctx.ready();
                ctx.stopped().await;
                Ok(())
            },
        )])
    }
}
