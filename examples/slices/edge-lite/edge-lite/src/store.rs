//! The store: keeps the units' readings and appends them to `data/readings.jsonl` every
//! `store.every`, and once more as the service stops. That last write depends on the stop
//! deadline: it ends shortly before the deadline and logs what it left, so that a slow disk never
//! makes the run outlive its deadline.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType, OrErr};
use rivium::{Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::units::{Faults, Reading};

/// The readings cannot be written.
const STORE: ErrorType = &ErrorKind::new("StoreError", Class::Internal);
/// What the last write leaves before the deadline, for the service to end.
const MARGIN: Duration = Duration::from_millis(200);

/// `[store]`: how often the readings are written.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreSettings {
    /// The period.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 86400>"
    )]
    pub every: Duration,
}

impl Default for StoreSettings {
    fn default() -> Self {
        StoreSettings {
            every: Duration::from_secs(10),
        }
    }
}

/// The `store` service, writing below `data`.
pub(crate) fn store(
    settings: &StoreSettings,
    faults: &Faults,
    data: PathBuf,
    mut received: mpsc::Receiver<Reading>,
) -> Box<dyn Service> {
    let (every, delay) = (settings.every, Duration::from_millis(faults.flush_delay_ms));
    rivium::service("store", ServiceKind::Background, move |ctx| async move {
        std::fs::create_dir_all(&data)
            .or_err_with(STORE, || format!("creating {}", data.display()))?;
        let file = data.join("readings.jsonl");
        let mut pending = Vec::new();
        let mut ticks = tokio::time::interval_at(Instant::now() + every, every);
        ctx.ready();
        let mut stopped = pin!(ctx.stopped());
        loop {
            tokio::select! {
                biased;
                () = &mut stopped => break,
                Some(reading) = received.recv() => pending.push(reading),
                _ = ticks.tick() => {
                    write(&file, &mut pending, delay, None).await?;
                }
            }
        }
        while let Ok(reading) = received.try_recv() {
            pending.push(reading);
        }
        let until = ctx.deadline().map(|deadline| deadline - MARGIN);
        let written = pending.len();
        let left = write(&file, &mut pending, delay, until).await?;
        if left > 0 {
            tracing::warn!(
                written = written - left,
                left,
                "the stop deadline cut the last write short"
            );
        } else {
            tracing::info!(written, "wrote the last readings");
        }
        Ok(())
    })
}

/// Appends `pending` to `file`, a line each, until `until`; returns how many it left.
async fn write(
    file: &Path,
    pending: &mut Vec<Reading>,
    delay: Duration,
    until: Option<Instant>,
) -> Result<usize> {
    if pending.is_empty() {
        return Ok(0);
    }
    let mut out = (OpenOptions::new().create(true).append(true).open(file))
        .or_err_with(STORE, || format!("opening {}", file.display()))?;
    let mut written = 0;
    for reading in pending.iter() {
        // A slow disk, when the faults ask for one.
        let slow = tokio::time::sleep(delay);
        match until {
            Some(until) => {
                if tokio::time::timeout_at(until, slow).await.is_err() {
                    break;
                }
            }
            None => slow.await,
        }
        let line = serde_json::to_string(reading).or_err(STORE, "encoding a reading")?;
        writeln!(out, "{line}").or_err_with(STORE, || format!("writing {}", file.display()))?;
        written += 1;
    }
    pending.drain(..written);
    Ok(pending.len())
}
