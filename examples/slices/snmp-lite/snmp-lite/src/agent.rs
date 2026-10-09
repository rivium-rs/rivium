//! The SNMP agent: a frontline service that answers GET and GETNEXT over UDP from a blocking
//! socket loop.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use rivium::error::{Class, ErrorKind, ErrorType, OrErr};
use rivium::{Result, Service, ServiceKind, StopSignal};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::mib::{Mib, Readings};
use crate::snmp::Message;

/// The socket cannot be bound or read.
const SOCKET: ErrorType = &ErrorKind::new("SocketError", Class::Unavailable);

/// `[snmp]`: where requests arrive and the community that may read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnmpSettings {
    /// Where requests arrive.
    pub addr: SocketAddr,
    /// The community string that requests must carry.
    pub community: String,
    /// How long each blocking read waits: keep it below `lifecycle.stop_timeout`.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub read_timeout: Duration,
}

impl Default for SnmpSettings {
    fn default() -> Self {
        SnmpSettings {
            // The SNMP port, as deployed; tests pass another one.
            addr: SocketAddr::from(([0, 0, 0, 0], 161)),
            community: "public".to_string(),
            read_timeout: Duration::from_secs(1),
        }
    }
}

/// The `agent` service.
pub(crate) fn agent(
    settings: &SnmpSettings,
    description: String,
    serial: String,
    readings: &Readings,
) -> Box<dyn Service> {
    let SnmpSettings {
        addr,
        community,
        read_timeout,
    } = settings.clone();
    let readings = readings.clone();
    rivium::service("agent", ServiceKind::Frontline, move |ctx| async move {
        // Binding fails the startup: the service is not ready yet.
        let socket = UdpSocket::bind(addr).or_err_with(SOCKET, || format!("binding {addr}"))?;
        socket
            .set_read_timeout(Some(read_timeout))
            .or_err(SOCKET, "setting the read timeout")?;
        ctx.listening(socket.local_addr().or_err(SOCKET, "reading the address")?);
        let mib = Mib {
            description,
            serial,
            readings,
            started: Instant::now(),
        };
        let stop = ctx.stop_signal();
        // Ready once the socket is read: a stop asked earlier would find no read to wait for.
        let (reading, read) = oneshot::channel();
        ctx.spawn_blocking("socket", move || {
            serve(&socket, community.as_bytes(), &mib, &stop, reading)
        });
        if read.await.is_ok() {
            ctx.ready();
        }
        ctx.stopped().await;
        Ok(())
    })
}

/// Answers requests until the service is asked to stop, which it notices between reads.
fn serve(
    socket: &UdpSocket,
    community: &[u8],
    mib: &Mib,
    stop: &StopSignal,
    reading: oneshot::Sender<()>,
) -> Result<()> {
    use std::io::ErrorKind::{ConnectionReset, Interrupted, TimedOut, WouldBlock};
    let mut buffer = [0; 4096];
    let mut reading = Some(reading);
    while !stop.is_stopping() {
        if let Some(reading) = reading.take() {
            let _ = reading.send(());
        }
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
            Err(error) => return Err(error).or_err(SOCKET, "receiving a request"),
        };
        let answer = match Message::decode(&buffer[..len]) {
            Ok(request) => mib.answer(&request, community),
            Err(error) => {
                tracing::debug!(%peer, %error, "dropped a datagram");
                None
            }
        };
        // An answer that cannot be sent is lost, as datagrams are.
        if let Some(answer) = answer
            && let Err(error) = socket.send_to(&answer.encode(), peer)
        {
            tracing::debug!(%peer, %error, "cannot answer");
        }
    }
    Ok(())
}
