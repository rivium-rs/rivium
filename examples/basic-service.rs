//! A process with one supervised service. Press Ctrl-C to stop gracefully.

use rivium::{Kind, Result, Service, service};
use rivium_host::{App, Code, Context};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Config {}

struct BasicService;

impl App for BasicService {
    const NAME: &'static str = "basic-service";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(_: &Config, _: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![service("worker", Kind::Frontline, |ctx| async move {
            ctx.ready();
            println!("Service ready; press Ctrl-C to stop.");
            ctx.stopped().await;
            println!("Service stopped.");
            Ok(())
        })])
    }
}

fn main() -> Code {
    rivium_host::process::run::<BasicService>()
}
