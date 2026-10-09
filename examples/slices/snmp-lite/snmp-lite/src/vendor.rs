//! The vendor's device: a simulated sensor read with a blocking call, as a serial line is. The
//! `device` service owns it in a blocking task; the `poll` job asks it for a reading every
//! `vendor.every` and records the answer.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use rivium::error::{Class, Error, ErrorKind, ErrorType};
use rivium::{Service, ServiceKind, StopSignal};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::mib::Readings;

/// The device is gone.
const DEVICE: ErrorType = &ErrorKind::new("DeviceError", Class::Unavailable);

/// A request for a reading, and where its answer goes.
type Request = oneshot::Sender<io::Result<i64>>;

/// `[vendor]`: the device and how often it is polled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VendorSettings {
    /// How often.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 3600>"
    )]
    pub every: Duration,
    /// How long a read waits for the device: keep it below `lifecycle.stop_timeout`.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 60>"
    )]
    pub read_timeout: Duration,
    /// How long the simulated device takes to answer, in milliseconds.
    pub latency_ms: u64,
}

impl Default for VendorSettings {
    fn default() -> Self {
        VendorSettings {
            every: Duration::from_secs(1),
            read_timeout: Duration::from_secs(1),
            latency_ms: 20,
        }
    }
}

/// The `device` service, and where the poller sends its requests.
pub(crate) fn device(settings: &VendorSettings) -> (Box<dyn Service>, Sender<Request>) {
    let (requests, queue) = std::sync::mpsc::channel();
    let (latency, timeout) = (
        Duration::from_millis(settings.latency_ms),
        settings.read_timeout,
    );
    let service = rivium::service("device", ServiceKind::Background, move |ctx| async move {
        let stop = ctx.stop_signal();
        let (started, start) = oneshot::channel();
        ctx.spawn_blocking("port", move || {
            let _ = started.send(());
            serve(&queue, &stop, latency, timeout);
            Ok(())
        });
        if start.await.is_ok() {
            ctx.ready();
        }
        ctx.stopped().await;
        Ok(())
    });
    (service, requests)
}

/// Answers requests until the service is asked to stop, which it notices between them.
fn serve(queue: &Receiver<Request>, stop: &StopSignal, latency: Duration, timeout: Duration) {
    let mut reads = 0;
    while !stop.is_stopping() {
        let answer = match queue.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        // The blocking read: the device answers after its latency, unless the read times out.
        std::thread::sleep(latency.min(timeout));
        let reading = match latency <= timeout {
            true => Ok(200 + reads % 50),
            false => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the device did not answer",
            )),
        };
        reads += 1;
        let _ = answer.send(reading);
    }
}

/// The `poll` job: asks the device for a reading and records it. A read that fails is counted
/// and logged; the next poll tries again.
pub(crate) fn poller(
    settings: &VendorSettings,
    requests: Sender<Request>,
    readings: &Readings,
) -> Box<dyn Service> {
    // The device answers within its read timeout once it has the request.
    let wait = settings.read_timeout + Duration::from_secs(1);
    let readings = readings.clone();
    rivium::periodic("poll", settings.every, move || {
        let (requests, readings) = (requests.clone(), readings.clone());
        async move {
            let (request, answer) = oneshot::channel();
            if requests.send(request).is_err() {
                return Error::e_explain(DEVICE, "the device service has ended");
            }
            match tokio::time::timeout(wait, answer).await {
                Ok(Ok(Ok(reading))) => readings.record(reading),
                Ok(Ok(Err(error))) => {
                    readings.fail();
                    tracing::warn!(%error, "no reading from the device");
                }
                _ => {
                    readings.fail();
                    tracing::warn!("no answer from the device in time");
                }
            }
            Ok(())
        }
    })
}
