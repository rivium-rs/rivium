//! Basic host that starts and stops cleanly.

use rivium::{App, AppContext, BoxFuture, Result, Service, ServiceContext};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Default)]
struct TestHost;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

struct NoopService;

impl Service for NoopService {
    fn name(&self) -> Cow<'static, str> {
        "noop".into()
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        })
    }
}

impl App for TestHost {
    const NAME: &'static str = "test-host";
    const VERSION: &'static str = "0.0.0";
    type Config = Config;

    fn services(_config: &Self::Config, _ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![Box::new(NoopService)])
    }
}

/// Entry point for the test-host binary.
pub fn main() -> std::process::ExitCode {
    rivium::process::run::<TestHost>()
}
