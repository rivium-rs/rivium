//! Reload selected settings through the host; keep startup-only settings unchanged.

use std::time::Duration;

use rivium::{Error, Kind, Result, Service, service};
use rivium_host::{App, Code, Context};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    message: String,
    interval_ms: u64,
    prefix: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            message: "Hello from Rivium".into(),
            interval_ms: 1000,
            prefix: "[demo]".into(),
        }
    }
}

struct ReloadApp;

impl App for ReloadApp {
    const NAME: &'static str = "config-reload";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn validate(config: &Config) -> Result<()> {
        if !(100..=60_000).contains(&config.interval_ms) {
            return Err(Error::invalid("interval_ms must be between 100 and 60000"));
        }
        Ok(())
    }

    fn reload_config(current: &Config, candidate: &Config) -> Result<Option<Config>> {
        let mut effective = current.clone();
        effective.message.clone_from(&candidate.message);
        effective.interval_ms = candidate.interval_ms;
        Ok(Some(effective))
    }

    fn services(config: &Config, host: &Context<Config>) -> Result<Vec<Box<dyn Service>>> {
        let mut settings = host.config();
        let prefix = config.prefix.clone();
        let host = host.clone();
        Ok(vec![
            service("messages", Kind::Frontline, move |ctx| async move {
                ctx.ready();
                loop {
                    let current = settings.current();
                    tokio::select! {
                        _ = ctx.stopped() => break,
                        changed = settings.changed() => {
                            if changed.is_none() {
                                break;
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(current.interval_ms)) => {
                            println!("{prefix} {}", current.message);
                        }
                    }
                }
                Ok(())
            }),
            service("reload", Kind::Background, move |ctx| async move {
                let period = Duration::from_secs(1);
                let mut poll =
                    tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                ctx.ready();
                loop {
                    tokio::select! {
                        _ = ctx.stopped() => break,
                        _ = poll.tick() => match host.reload().await {
                            Ok(report) => {
                                if !report.published.is_empty() || !report.requires_restart.is_empty() {
                                    println!(
                                        "Generation {}: published {:?}; requires restart {:?}",
                                        report.generation, report.published, report.requires_restart,
                                    );
                                }
                            }
                            Err(error) => eprintln!("Keeping current configuration: {error}"),
                        }
                    }
                }
                Ok(())
            }),
        ])
    }
}

fn main() -> Code {
    rivium_host::process::run::<ReloadApp>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_policy_preserves_startup_only_settings() {
        let current = Config::default();
        let candidate = Config {
            message: "Updated message".into(),
            interval_ms: 250,
            prefix: "[next run]".into(),
        };

        let effective = ReloadApp::reload_config(&current, &candidate)
            .unwrap()
            .unwrap();

        assert_eq!(effective.message, candidate.message);
        assert_eq!(effective.interval_ms, candidate.interval_ms);
        assert_eq!(effective.prefix, current.prefix);
    }
}
