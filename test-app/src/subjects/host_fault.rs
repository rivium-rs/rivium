//! Host that fails in different phases (build, ready, run).

use rivium::{
    error::{kinds, Error},
    App, AppContext, Result,
};
use serde::{Deserialize, Serialize};
use std::env;

struct FaultHost;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

#[derive(Clone, Copy)]
enum FaultMode {
    Build,
    Ready,
    Run,
}

impl FaultMode {
    fn from_env() -> Self {
        match env::var("TEST_FAULT_MODE").as_deref() {
            Ok("build") => FaultMode::Build,
            Ok("ready") => FaultMode::Ready,
            Ok("run") => FaultMode::Run,
            _ => FaultMode::Build,
        }
    }
}

impl App for FaultHost {
    const NAME: &'static str = "fault-host";
    const VERSION: &'static str = "0.0.0";
    type Config = Config;

    fn services(
        _config: &Self::Config,
        _ctx: &AppContext,
    ) -> Result<Vec<Box<dyn rivium::Service>>> {
        let mode = FaultMode::from_env();

        if matches!(mode, FaultMode::Build) {
            return Err(Error::explain(kinds::INTERNAL, "build failure"));
        }

        let svc = rivium::service(
            "fault",
            rivium::ServiceKind::Background,
            move |ctx| async move {
                if matches!(mode, FaultMode::Ready) {
                    return Err(Error::explain(kinds::INTERNAL, "ready failure"));
                }

                ctx.ready();

                if matches!(mode, FaultMode::Run) {
                    return Err(Error::explain(kinds::INTERNAL, "run failure"));
                }

                ctx.stopped().await;
                Ok(())
            },
        );

        Ok(vec![svc])
    }
}

/// Entry point for the fault-host binary in build-failure mode.
pub fn main_build() -> std::process::ExitCode {
    std::env::set_var("TEST_FAULT_MODE", "build");
    rivium::process::run::<FaultHost>()
}

/// Entry point for the fault-host binary in ready-failure mode.
pub fn main_ready() -> std::process::ExitCode {
    std::env::set_var("TEST_FAULT_MODE", "ready");
    rivium::process::run::<FaultHost>()
}

/// Entry point for the fault-host binary in run-failure mode.
pub fn main_run() -> std::process::ExitCode {
    std::env::set_var("TEST_FAULT_MODE", "run");
    rivium::process::run::<FaultHost>()
}
