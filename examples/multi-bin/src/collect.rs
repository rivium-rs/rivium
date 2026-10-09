//! The collector's job: asks each station for its reading, periodically, and logs it.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use rivium::Service;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

/// `[collect]`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectSettings {
    /// The stations' query addresses.
    pub stations: Vec<SocketAddr>,
    /// How often.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 86400>"
    )]
    pub every: Duration,
    /// How long to wait for each answer.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub timeout: Duration,
}

impl Default for CollectSettings {
    fn default() -> Self {
        CollectSettings {
            stations: Vec::new(),
            every: Duration::from_secs(10),
            timeout: Duration::from_secs(1),
        }
    }
}

/// The job: a background service. A station that does not answer is logged and asked again
/// next time; it does not fail the service.
#[must_use]
pub fn collect(settings: &CollectSettings) -> Box<dyn Service> {
    let CollectSettings {
        stations,
        every,
        timeout,
    } = settings.clone();
    rivium::periodic("collect", every, move || {
        let stations = stations.clone();
        async move {
            for station in stations {
                match ask(station, timeout).await {
                    Ok(value) => tracing::info!(%station, value, "reading"),
                    Err(error) => tracing::warn!(%station, %error, "no reading"),
                }
            }
            Ok(())
        }
    })
}

/// Asks `station` for its reading.
async fn ask(station: SocketAddr, timeout: Duration) -> io::Result<String> {
    let any = match station {
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(_) => SocketAddr::from(([0; 16], 0)),
    };
    let socket = UdpSocket::bind(any).await?;
    socket.send_to(b"?", station).await?;
    let mut buffer = [0; 64];
    let (len, _) = tokio::time::timeout(timeout, socket.recv_from(&mut buffer))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok(String::from_utf8_lossy(&buffer[..len]).into_owned())
}
