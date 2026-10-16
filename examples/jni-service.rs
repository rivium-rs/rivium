//! A JNI example library for the Java bridge in java/rivium/examples/Bridge.java.

#![forbid(unsafe_code)]

use rivium::{Kind, Result, Service, service};
use rivium_host::{App, Context};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Config {}

struct JniApp;

impl App for JniApp {
    const NAME: &'static str = "jni-example";
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

rivium_jni::export!(class = "rivium/examples/Bridge", app = JniApp);
