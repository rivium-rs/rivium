//! The query service: answers every datagram with the latest reading, as text.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType, OrErr};
use rivium::{Result, Service, ServiceKind, StopSignal};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::meter::Reading;

/// The socket cannot be bound or read.
const SOCKET: ErrorType = &ErrorKind::new("SocketError", Class::Unavailable);

/// `[query]`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuerySettings {
    /// Where queries arrive.
    pub addr: SocketAddr,
    /// How long each blocking read waits: the service notices a stop request within this
    /// time, so keep it below `lifecycle.stop_timeout`.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub read_timeout: Duration,
}

impl Default for QuerySettings {
    fn default() -> Self {
        QuerySettings {
            // Any free port: the tests run several programs at once.
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            read_timeout: Duration::from_secs(1),
        }
    }
}

/// The service: a frontline one, which takes the stations' queries.
#[must_use]
pub fn query(settings: &QuerySettings, reading: &Reading) -> Box<dyn Service> {
    let QuerySettings { addr, read_timeout } = settings.clone();
    let reading = reading.clone();
    rivium::service("query", ServiceKind::Frontline, move |ctx| async move {
        // Binding fails the startup: the service is not ready yet.
        let socket = UdpSocket::bind(addr).or_err_with(SOCKET, || format!("binding {addr}"))?;
        socket
            .set_read_timeout(Some(read_timeout))
            .or_err(SOCKET, "setting the read timeout")?;
        ctx.listening(socket.local_addr().or_err(SOCKET, "reading the address")?);
        let stop = ctx.stop_signal();
        // Ready once the socket is read: a stop asked earlier would find no read to wait for.
        let (reading_now, read) = oneshot::channel();
        ctx.spawn_blocking("socket", move || {
            answer(&socket, &reading, &stop, reading_now)
        });
        if read.await.is_ok() {
            ctx.ready();
        }
        ctx.stopped().await;
        Ok(())
    })
}

/// Answers queries until the service is asked to stop, which it notices between reads.
fn answer(
    socket: &UdpSocket,
    reading: &Reading,
    stop: &StopSignal,
    reading_now: oneshot::Sender<()>,
) -> Result<()> {
    use std::io::ErrorKind::{ConnectionReset, Interrupted, TimedOut, WouldBlock};
    let mut buffer = [0; 512];
    let mut reading_now = Some(reading_now);
    while !stop.is_stopping() {
        if let Some(reading_now) = reading_now.take() {
            let _ = reading_now.send(());
        }
        let peer = match socket.recv_from(&mut buffer) {
            Ok((_, peer)) => peer,
            // A timeout; a signal, which on Linux interrupts a read with a timeout even with
            // SA_RESTART; or on Windows a peer that went away before an earlier answer.
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
        // An answer that cannot be sent is lost, as datagrams are.
        let answer = format!("{:.1}", reading.get());
        if let Err(error) = socket.send_to(answer.as_bytes(), peer) {
            tracing::debug!(%peer, %error, "cannot answer");
        }
    }
    Ok(())
}
