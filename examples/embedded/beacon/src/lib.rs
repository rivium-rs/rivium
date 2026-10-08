//! Beacon: a UDP service that answers every datagram with the device's serial number and how
//! many queries it has answered, and logs a heartbeat. It is the service library of the
//! embedded example, written once for both hosts: `beacon-bin` runs it as a program of its
//! own, `beacon-jni` inside an Android app, which passes the device's serial number with
//! `--set beacon.serial=…`.
//!
//! ```toml
//! [beacon]
//! addr = "127.0.0.1:0"   # any free port; the `listening` event says which
//! read_timeout = "1s"    # each blocking read waits at most this long
//! serial = "unknown"
//!
//! [heartbeat]
//! every = "1m"
//! ```

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType, OrErr};
use rivium::{App, AppContext, Result, Service, ServiceKind, StopSignal};
use serde::{Deserialize, Serialize};

/// The socket cannot be bound or read.
const SOCKET: ErrorType = &ErrorKind::new("SocketError", Class::Unavailable);

/// The program, for both hosts.
pub struct Beacon;

impl App for Beacon {
    const NAME: &'static str = "beacon";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let answered = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&answered);
        let heartbeat = rivium::periodic("heartbeat", config.heartbeat.every, move || {
            let answered = counted.load(Relaxed);
            async move {
                tracing::info!(answered, "heartbeat");
                Ok(())
            }
        });
        let BeaconSettings {
            addr,
            read_timeout,
            serial,
        } = config.beacon.clone();
        let beacon = rivium::service("beacon", ServiceKind::Frontline, move |ctx| async move {
            // Binding fails the startup: the service is not ready yet.
            let socket = UdpSocket::bind(addr).or_err_with(SOCKET, || format!("binding {addr}"))?;
            socket
                .set_read_timeout(Some(read_timeout))
                .or_err(SOCKET, "setting the read timeout")?;
            ctx.listening(socket.local_addr().or_err(SOCKET, "reading the address")?);
            let stop = ctx.stop_signal();
            ctx.spawn_blocking("socket", move || answer(&socket, &serial, &stop, &answered));
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        });
        Ok(vec![beacon, heartbeat])
    }
}

/// The configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `[beacon]`
    pub beacon: BeaconSettings,
    /// `[heartbeat]`
    pub heartbeat: HeartbeatSettings,
}

/// `[beacon]`: the socket and what the answers say.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BeaconSettings {
    /// Where queries arrive.
    pub addr: SocketAddr,
    /// How long each blocking read waits: the service notices a stop request within this
    /// time, so keep it below `lifecycle.stop_timeout`.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub read_timeout: Duration,
    /// The device's serial number, which the host passes.
    pub serial: String,
}

impl Default for BeaconSettings {
    fn default() -> Self {
        BeaconSettings {
            // Any free port: tests run several instances at once.
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            read_timeout: Duration::from_secs(1),
            serial: "unknown".to_string(),
        }
    }
}

/// `[heartbeat]`: how often the count of answers is logged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatSettings {
    /// The period.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 86400>"
    )]
    pub every: Duration,
}

impl Default for HeartbeatSettings {
    fn default() -> Self {
        HeartbeatSettings {
            every: Duration::from_secs(60),
        }
    }
}

/// Answers queries with `<serial> <count>` until the service is asked to stop, which it notices
/// between reads.
fn answer(socket: &UdpSocket, serial: &str, stop: &StopSignal, answered: &AtomicU64) -> Result<()> {
    use std::io::ErrorKind::{ConnectionReset, Interrupted, TimedOut, WouldBlock};
    let mut buffer = [0; 512];
    while !stop.is_stopping() {
        let peer = match socket.recv_from(&mut buffer) {
            Ok((_, peer)) => peer,
            // A timeout; a signal, which on Linux interrupts a read with a timeout even with
            // SA_RESTART, as does stopping and continuing the process; or on Windows a peer
            // that went away before an earlier answer.
            Err(error)
                if matches!(
                    error.kind(),
                    WouldBlock | TimedOut | Interrupted | ConnectionReset
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).or_err(SOCKET, "receiving a query"),
        };
        let count = answered.fetch_add(1, Relaxed) + 1;
        // An answer that cannot be sent is lost, as datagrams are.
        if let Err(error) = socket.send_to(format!("{serial} {count}").as_bytes(), peer) {
            tracing::debug!(%peer, %error, "cannot answer");
        }
    }
    Ok(())
}
