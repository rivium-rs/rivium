//! Host with log-export feature (placeholder for V-2).

use rivium::{App, AppContext, Result};
use serde::{Deserialize, Serialize};

#[derive(Default)]
struct ExportHost;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

impl App for ExportHost {
    const NAME: &'static str = "export-host";
    const VERSION: &'static str = "0.0.0";
    type Config = Config;

    fn services(
        _config: &Self::Config,
        _ctx: &AppContext,
    ) -> Result<Vec<Box<dyn rivium::Service>>> {
        // Will emit structured logs when log-export feature is enabled.
        // For now, just a placeholder.
        Ok(vec![])
    }
}

/// Entry point for the export-host binary.
pub fn main() -> std::process::ExitCode {
    rivium::process::run::<ExportHost>()
}
