//! A UDP echo service: it sends every datagram back to its sender, and logs how many it has
//! echoed. It shows a service that is not an HTTP server, blocking I/O inside a service, a
//! periodic job, a restart request: the datagram `restart` asks for one, and log export without
//! HTTP: the datagram `export` packs today's logs into an archive in the log directory, which
//! the `log export finished` event names.
//!
//! ```toml
//! [echo]
//! addr = "127.0.0.1:0"   # any free port; the `listening` event says which
//! read_timeout = "1s"    # each blocking read waits at most this long
//!
//! [stats]
//! every = "1m"
//! ```

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType, OrErr, log_error};
use rivium::log::{Date, ExportRequest, LogExporter};
use rivium::{App, AppContext, Restarter, Result, Service, ServiceKind, StopSignal};
use serde::{Deserialize, Serialize};

/// The socket cannot be bound or read.
const SOCKET: ErrorType = &ErrorKind::new("SocketError", Class::Unavailable);

/// The program.
pub struct Echo;

impl App for Echo {
    const NAME: &'static str = "udp-echo";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        Ok(services(&config.echo, &config.stats, ctx.log_exporter()))
    }
}

/// The configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `[echo]`
    pub echo: EchoSettings,
    /// `[stats]`
    pub stats: StatsSettings,
}

/// `[echo]`: the socket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EchoSettings {
    /// Where datagrams arrive.
    pub addr: SocketAddr,
    /// How long each blocking read waits: the service notices a stop request within this
    /// time, so keep it below `lifecycle.stop_timeout`.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub read_timeout: Duration,
}

impl Default for EchoSettings {
    fn default() -> Self {
        EchoSettings {
            // Any free port: the tests run several instances at once.
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            read_timeout: Duration::from_secs(1),
        }
    }
}

/// `[stats]`: how often the count of echoed datagrams is logged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatsSettings {
    /// The period.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 86400>"
    )]
    pub every: Duration,
}

impl Default for StatsSettings {
    fn default() -> Self {
        StatsSettings {
            every: Duration::from_secs(60),
        }
    }
}

/// The services: `echo`, which serves the socket, and `stats`, which logs its count.
#[must_use]
pub fn services(
    echo: &EchoSettings,
    stats: &StatsSettings,
    exporter: &LogExporter,
) -> Vec<Box<dyn Service>> {
    let echoed = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&echoed);
    let stats = rivium::periodic("stats", stats.every, move || {
        let echoed = counted.load(Relaxed);
        async move {
            tracing::info!(echoed, "datagrams echoed");
            Ok(())
        }
    });
    let EchoSettings { addr, read_timeout } = echo.clone();
    let exporter = exporter.clone();
    let echo = rivium::service("echo", ServiceKind::Frontline, move |ctx| async move {
        // Binding fails the startup: the service is not ready yet.
        let socket = UdpSocket::bind(addr).or_err_with(SOCKET, || format!("binding {addr}"))?;
        socket
            .set_read_timeout(Some(read_timeout))
            .or_err(SOCKET, "setting the read timeout")?;
        let bound = socket.local_addr().or_err(SOCKET, "reading the address")?;
        ctx.listening(bound);
        let (stop, restarter) = (ctx.stop_signal(), ctx.restarter());
        ctx.spawn_blocking("socket", move || {
            serve(&socket, &stop, &restarter, &exporter, &echoed)
        });
        ctx.ready();
        ctx.stopped().await;
        Ok(())
    });
    vec![echo, stats]
}

/// Echoes datagrams until the service is asked to stop, which it notices between reads.
fn serve(
    socket: &UdpSocket,
    stop: &StopSignal,
    restarter: &Restarter,
    exporter: &LogExporter,
    echoed: &AtomicU64,
) -> Result<()> {
    use std::io::ErrorKind::{ConnectionReset, Interrupted, TimedOut, WouldBlock};
    let mut buffer = [0; 65_536];
    while !stop.is_stopping() {
        let (len, peer) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
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
            Err(error) => return Err(error).or_err(SOCKET, "receiving a datagram"),
        };
        let datagram = &buffer[..len];
        let text = String::from_utf8_lossy(&datagram[..len.min(64)]);
        tracing::debug!(%peer, len, %text, "datagram");
        if datagram == b"restart" {
            restarter.request(format!("asked by {peer}"));
            continue;
        }
        if datagram == b"export" {
            // The exporter logs how the export goes; one export runs at a time.
            let (from, to) = (Date::today(), Date::today());
            let request = ExportRequest {
                from,
                to,
                sinks: Vec::new(),
            };
            if let Err(error) = exporter.start(request) {
                log_error!(tracing::Level::WARN, &error, %peer, "cannot export the logs");
            }
            continue;
        }
        // An answer that cannot be sent is lost, as datagrams are.
        if let Err(error) = socket.send_to(datagram, peer) {
            tracing::debug!(%peer, %error, "cannot answer");
        }
        echoed.fetch_add(1, Relaxed);
    }
    Ok(())
}
