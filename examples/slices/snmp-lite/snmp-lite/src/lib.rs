//! snmp-lite: a validation slice of an SNMP agent that runs as a program of its own and inside
//! an Android app. It answers SNMPv2c GET and GETNEXT for a few scalars over UDP, among them the
//! device's serial number, which the app passes with `--set device.serial=…`, and the last
//! reading of a vendor's device, which a job polls with a blocking read. This service library
//! is written once for both hosts: `snmp-lite-bin` runs it as a program, `snmp-lite-jni` in an
//! app.
//!
//! ```toml
//! [snmp]
//! addr = "0.0.0.0:161"   # tests and apps pass another port
//! community = "public"
//! read_timeout = "1s"    # each blocking read waits at most this long
//!
//! [device]
//! serial = "unknown"
//!
//! [vendor]
//! every = "1s"
//! read_timeout = "1s"
//! latency_ms = 20        # how long the simulated device takes to answer
//! ```

use rivium::{App, AppContext, Result, Service};
use serde::{Deserialize, Serialize};

mod agent;
pub mod mib;
pub mod snmp;
mod vendor;

pub use agent::SnmpSettings;
pub use vendor::VendorSettings;

/// The program, for both hosts.
pub struct SnmpLite;

impl App for SnmpLite {
    const NAME: &'static str = "snmp-lite";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let readings = mib::Readings::default();
        let description = format!("{} {}", Self::NAME, Self::VERSION);
        let (device, requests) = vendor::device(&config.vendor);
        Ok(vec![
            agent::agent(
                &config.snmp,
                description,
                config.device.serial.clone(),
                &readings,
            ),
            device,
            vendor::poller(&config.vendor, requests, &readings),
        ])
    }
}

/// The configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `[snmp]`
    pub snmp: SnmpSettings,
    /// `[device]`
    pub device: DeviceSettings,
    /// `[vendor]`
    pub vendor: VendorSettings,
}

/// `[device]`: what the host knows about the device it runs on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSettings {
    /// The serial number, which the host passes.
    pub serial: String,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        DeviceSettings {
            serial: "unknown".to_string(),
        }
    }
}
