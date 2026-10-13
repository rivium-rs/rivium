//! Application shared by the in-process and dynamic-library JVM contract tests.

use rivium::{App, AppContext, Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};

/// A small embedded application with a controllable stop for timeout checks.
pub struct JvmApp;

/// Configuration used only by the JVM contract's unresponsive-service scenario.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    ignore_stop: bool,
}

impl App for JvmApp {
    const NAME: &'static str = "jvm-host";
    const VERSION: &'static str = "0.0.0";
    type Config = Config;

    fn services(config: &Config, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let ignore_stop = config.ignore_stop;
        Ok(vec![rivium::service(
            "probe",
            ServiceKind::Background,
            move |ctx| async move {
                ctx.ready();
                if ignore_stop {
                    std::future::pending::<()>().await;
                } else {
                    ctx.stopped().await;
                }
                Ok(())
            },
        )])
    }
}
