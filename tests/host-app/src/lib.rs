//! A private, non-HTTP application for testing host boundaries, not a teaching example.
#![forbid(unsafe_code)]

use rivium::{App, AppContext, Service, ServiceKind};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// The minimal application shared by host subjects.
pub struct VerifyApp;

/// Business configuration; host-owned settings remain outside this type.
#[derive(Default, Deserialize, Serialize)]
pub struct Config {
    /// Deterministic verification inputs.
    pub verify: Verify,
}

/// Finite boundary cases; not a script language or a product feature.
#[derive(Deserialize, Serialize)]
pub struct Verify {
    /// The selected boundary case.
    #[serde(deserialize_with = "case")]
    pub case: String,
}

impl Default for Verify {
    fn default() -> Self {
        Self {
            case: "idle".into(),
        }
    }
}

impl App for VerifyApp {
    const NAME: &'static str = "rivium-verify";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, app: &AppContext) -> rivium::Result<Vec<Box<dyn Service>>> {
        let case = config.verify.case.clone();
        match case.as_str() {
            "construct-error" => return Err(failure("construction failed")),
            "construct-panic" => panic!("construction panic"),
            "empty" => return Ok(vec![]),
            _ => {}
        }
        let root = app.paths().root().display().to_string();
        let resolved = app.paths().resolve("relative.txt").display().to_string();
        let restarter = app.restarter().clone();
        let kind = if case == "frontline-return" {
            ServiceKind::Frontline
        } else {
            ServiceKind::Background
        };
        let mut services = vec![rivium::service("control", kind, move |ctx| async move {
            match case.as_str() {
                "startup-error" => return Err(failure("startup failed")),
                "startup-panic" => panic!("startup panic"),
                _ => {}
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| {
                    rivium::Error::because(rivium::error::kinds::INTERNAL, "control bind", error)
                })?;
            ctx.listening(listener.local_addr().expect("bound address"));
            tracing::info!(pid = std::process::id(), root, resolved, "verify identity");
            ctx.ready();
            loop {
                tokio::select! {
                    _ = ctx.stopped() => {
                        tracing::info!("verify stopping");
                        if case == "stop-restart" {
                            restarter.request("after external stop");
                            tracing::info!("verify restart requested after stop");
                        }
                        if case == "hold-stop" {
                            // A deterministic barrier: no completion until the host abandons it.
                            std::future::pending::<()>().await;
                        }
                        tracing::info!("verify final");
                        return Ok(());
                    }
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.map_err(|error| rivium::Error::because(
                            rivium::error::kinds::INTERNAL, "control accept", error))?;
                        let mut byte = [0];
                        tokio::select! {
                            _ = ctx.stopped() => continue,
                            result = stream.read_exact(&mut byte) => {
                                if result.is_err() { continue; }
                            }
                        }
                        match byte[0] {
                            b'F' => { tracing::info!("verify final"); return Ok(()); }
                            b'E' => return Err(failure("runtime failed")),
                            b'P' => panic!("runtime panic"),
                            b'C' => ctx.spawn("failed-child", async { Err(failure("child failed")) }),
                            b'R' => restarter.request("verification request"),
                            _ => return Err(failure("unknown control byte")),
                        }
                    }
                }
            }
        })];
        if config.verify.case == "partly-ready" {
            services.push(rivium::service(
                "unready",
                ServiceKind::Background,
                |ctx| async move {
                    ctx.stopped().await;
                    Ok(())
                },
            ));
        }
        Ok(services)
    }
}

fn failure(message: &'static str) -> rivium::BError {
    rivium::Error::explain(rivium::error::kinds::INTERNAL, message)
}

fn case<'de, D: serde::Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    let value = String::deserialize(de)?;
    match value.as_str() {
        "idle" | "construct-error" | "construct-panic" | "empty" | "startup-error"
        | "startup-panic" | "hold-stop" | "stop-restart" | "partly-ready" | "frontline-return" => {
            Ok(value)
        }
        _ => Err(serde::de::Error::custom("invalid verification case")),
    }
}
