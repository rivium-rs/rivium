//! `udp-echo` with faults to inject, for the process tests and the hosting tests: the same
//! services, and `[faults]` to make the composition root panic or a service fail once it is
//! ready, in every run or in the first runs only.

use std::time::Duration;

use rivium::error::{Error, OrErr, kinds};
use rivium::{App, AppContext, Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};
use udp_echo::{EchoSettings, StatsSettings};

struct Faulty;

#[derive(Default, Serialize, Deserialize)]
struct Config {
    echo: EchoSettings,
    stats: StatsSettings,
    faults: Faults,
}

/// `[faults]`.
#[derive(Default, Serialize, Deserialize)]
struct Faults {
    /// Panic while building the services.
    panic_in_services: bool,
    /// Milliseconds after which a ready service fails; 0 for never.
    fail_after_ms: u64,
    /// With `fail_after_ms`, only the first runs fail, this many; 0 for every run. The runs are
    /// counted in the file `runs` below the root, across restarts of the program.
    fail_runs: u32,
}

impl App for Faulty {
    const NAME: &'static str = "udp-echo-faults";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let faults = &config.faults;
        assert!(
            !faults.panic_in_services,
            "a panic injected in the composition root"
        );
        let mut services = udp_echo::services(&config.echo, &config.stats, ctx.log_exporter());
        if faults.fail_after_ms > 0 && (faults.fail_runs == 0 || run(ctx)? <= faults.fail_runs) {
            let after = Duration::from_millis(faults.fail_after_ms);
            let fault = rivium::service("fault", ServiceKind::Background, move |ctx| async move {
                ctx.spawn_blocking("timer", move || {
                    std::thread::sleep(after);
                    Error::e_explain(kinds::INTERNAL, "a fault injected in a service")
                });
                ctx.ready();
                ctx.stopped().await;
                Ok(())
            });
            services.push(fault);
        }
        Ok(services)
    }
}

/// Counts this run in the file `runs` below the root and returns its number, from 1.
fn run(ctx: &AppContext) -> Result<u32> {
    let file = ctx.paths().resolve("runs");
    let runs = std::fs::read_to_string(&file).unwrap_or_default();
    let run = runs.trim().parse::<u32>().unwrap_or(0) + 1;
    std::fs::write(&file, run.to_string()).or_err(kinds::INTERNAL, "counting the runs")?;
    Ok(run)
}

fn main() -> std::process::ExitCode {
    rivium::process::run::<Faulty>()
}
