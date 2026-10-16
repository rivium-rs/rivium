//! Start and stop the same application twice without process signals.

use std::time::Duration;

use rivium::{Kind, Result, Service, service};
use rivium_host::{App, Code, Context, embedded::Host};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Config {}

struct EmbeddedApp;

impl App for EmbeddedApp {
    const NAME: &'static str = "embedded-example";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(_: &Config, _: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        Ok(vec![service("worker", Kind::Frontline, |ctx| async move {
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        })])
    }
}

fn main() -> Code {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/embedded-example".into());
    let args = ["--root".into(), root];
    let host = Host::new::<EmbeddedApp>();

    for round in 1..=2 {
        let code = host.start(&args);
        if code != Code::Ok {
            eprintln!("Start failed: {code:?}; {:?}", host.last_error());
            return code;
        }
        println!("Round {round}: {:?}", host.status());

        let code = host.stop(Duration::from_secs(2));
        if code != Code::Ok {
            eprintln!("Stop failed: {code:?}; {:?}", host.last_error());
            return code;
        }
        println!("Round {round}: {:?}", host.status());
    }
    Code::Ok
}
