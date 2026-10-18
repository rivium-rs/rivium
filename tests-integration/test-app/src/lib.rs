//! Fault-injection application shared by process, embedded and JNI contracts.

use rivium::{Kind, Result, Service, service};
use rivium_host::{App, Context};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: String,
}

pub struct TestApp;

impl App for TestApp {
    const NAME: &'static str = "rivium-demo";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, context: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        if config.mode == "build-error" {
            return Err(rivium::Error::internal("construction rejected"));
        }
        if config.mode == "build-panic" {
            panic!("construction panic");
        }
        if config.mode == "restart-backoff" {
            use std::io::Write;

            let mut builds = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(context.paths().root().join("builds"))?;
            writeln!(builds, "built")?;
        }
        let mode = config.mode.clone();
        let stopping = context.paths().root().join("stopping");
        let services = vec![service("worker", Kind::Frontline, move |ctx| async move {
            if mode == "startup-error" {
                return Err(rivium::Error::internal("startup rejected"));
            }
            if mode == "startup-hang" {
                std::future::pending::<()>().await;
            }
            ctx.ready();
            if matches!(mode.as_str(), "run-error" | "restart-backoff") {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                return Err(rivium::Error::internal("run failed"));
            }
            if mode == "restart" {
                ctx.control().restart("configuration changed");
            }
            ctx.stopped().await;
            if mode == "stop-hang" {
                std::fs::write(stopping, b"stopping")?;
                std::future::pending::<()>().await;
            }
            Ok(())
        })];
        Ok(services)
    }
}
