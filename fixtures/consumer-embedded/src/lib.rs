//! A consumer of Rivium that runs embedded: the same kind of services as a program's, and the
//! JNI library that an Android app loads.
#![forbid(unsafe_code)]

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType, OrErr};
use rivium::{App, AppContext, Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};

const SOCKET: ErrorType = &ErrorKind::new("SocketError", Class::Unavailable);

/// The program, as the app runs it.
pub struct Consumer;

impl App for Consumer {
    const NAME: &'static str = "consumer-embedded";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = Config;

    fn services(config: &Config, _: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let addr = config.addr;
        let answer = rivium::service("answer", ServiceKind::Frontline, move |ctx| async move {
            let socket = UdpSocket::bind(addr).or_err_with(SOCKET, || format!("binding {addr}"))?;
            (socket.set_read_timeout(Some(Duration::from_millis(500))))
                .or_err(SOCKET, "setting the read timeout")?;
            ctx.listening(socket.local_addr().or_err(SOCKET, "reading the address")?);
            let stop = ctx.stop_signal();
            ctx.spawn_blocking("socket", move || {
                let mut buffer = [0; 512];
                while !stop.is_stopping() {
                    if let Ok((len, peer)) = socket.recv_from(&mut buffer) {
                        let _ = socket.send_to(&buffer[..len], peer);
                    }
                }
                Ok(())
            });
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        });
        let tick = rivium::periodic("tick", Duration::from_secs(1), || async {
            tracing::debug!("tick");
            Ok(())
        });
        Ok(vec![answer, tick])
    }
}

/// The configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Where datagrams arrive.
    pub addr: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            addr: SocketAddr::from(([0, 0, 0, 0], 7701)),
        }
    }
}

rivium_jni::export!(class = "com/example/consumer/RiviumBridge", app = Consumer);
