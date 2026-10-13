//! Host that ignores the first signal (deadline scenario).

use rivium::{App, AppContext, BoxFuture, Result, Service, ServiceContext};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Default)]
struct DeadlineHost;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

struct IgnoringService;

impl Service for IgnoringService {
    fn name(&self) -> Cow<'static, str> {
        "ignoring".into()
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            ctx.ready();
            // Ignore the stop signal - sleep forever.
            std::future::pending::<()>().await;
            Ok(())
        })
    }
}

impl App for DeadlineHost {
    const NAME: &'static str = "deadline-host";
    const VERSION: &'static str = "0.0.0";
    type Config = Config;

    fn services(_config: &Self::Config, _ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![Box::new(IgnoringService)])
    }
}

/// Entry point for the deadline-host binary.
pub fn main() -> std::process::ExitCode {
    rivium::process::run::<DeadlineHost>()
}
