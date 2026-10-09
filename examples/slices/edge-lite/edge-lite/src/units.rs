//! The collector: units that poll simulated devices, supervised by the collector itself, as the
//! edge service's unit manager does. A unit that panics or fails is logged and started again a
//! poll period later; after `units.give_up_after` failures in a row, with no poll answered in
//! between, it stays down. Once every unit is down, the collector fails, and with it the run.
//!
//! The units run in the collector's own `JoinSet`, not as tasks of the service
//! (`ctx.spawn`): the first failure of such a task fails the service, and a unit's failure is the
//! collector's to handle.

use std::collections::HashMap;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rivium::error::{Class, Error, ErrorKind, ErrorType};
use rivium::{Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::{Id, JoinSet};
use tracing::Instrument;

/// A device does not answer.
const DEVICE: ErrorType = &ErrorKind::new("DeviceError", Class::Unavailable);
/// No unit is left.
const UNITS: ErrorType = &ErrorKind::new("UnitsGivenUp", Class::Unavailable);

/// `[units]`: how many units, how often they poll, and when one is given up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitSettings {
    /// How many units, one per device.
    #[serde(deserialize_with = "rivium::config::de::integer::<_, usize, 1, 64>")]
    pub count: usize,
    /// How often a unit polls its device, and how long a failed unit waits to start again.
    #[serde(
        serialize_with = "rivium::config::de::serialize_duration",
        deserialize_with = "rivium::config::de::duration::<_, 1, 3600>"
    )]
    pub every: Duration,
    /// Failures in a row after which a unit stays down.
    #[serde(deserialize_with = "rivium::config::de::integer::<_, u32, 1, 1000>")]
    pub give_up_after: u32,
}

impl Default for UnitSettings {
    fn default() -> Self {
        UnitSettings {
            count: 2,
            every: Duration::from_secs(1),
            give_up_after: 5,
        }
    }
}

/// `[faults]`: faults to inject, for tests and drills; none by default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Faults {
    /// A unit panics on every n-th poll of its run; 0 for never.
    pub panic_every: u64,
    /// A unit fails on every n-th poll of its run; 0 for never.
    pub error_every: u64,
    /// Writing each reading takes this many milliseconds.
    pub flush_delay_ms: u64,
}

/// A reading of a unit's device.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Reading {
    /// The unit.
    pub unit: usize,
    /// What the device read.
    pub value: i64,
    /// When, in milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// What a unit has done so far.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UnitState {
    /// From 0.
    pub id: usize,
    /// `running`, `restarting` or `given up`.
    pub state: String,
    /// Polls answered.
    pub polls: u64,
    /// Failures and panics.
    pub failures: u64,
    /// The last reading.
    pub last: Option<i64>,
    /// Failures since the last poll answered.
    #[serde(skip)]
    streak: u32,
}

/// The state of every unit, shared by the collector and the API.
#[derive(Clone, Debug)]
pub struct Status(Arc<Mutex<Vec<UnitState>>>);

impl Status {
    /// `count` units, about to run.
    #[must_use]
    pub fn new(count: usize) -> Self {
        let units = (0..count).map(|id| UnitState {
            id,
            state: "running".to_string(),
            ..UnitState::default()
        });
        Status(Arc::new(Mutex::new(units.collect())))
    }

    /// The state of every unit, now.
    #[must_use]
    pub fn snapshot(&self) -> Vec<UnitState> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn update<T>(&self, id: usize, change: impl FnOnce(&mut UnitState) -> T) -> T {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner)[id])
    }
}

/// The `collector` service.
pub(crate) fn collector(
    settings: &UnitSettings,
    faults: &Faults,
    status: &Status,
    readings: mpsc::Sender<Reading>,
) -> Box<dyn Service> {
    let (settings, faults, status) = (settings.clone(), faults.clone(), status.clone());
    rivium::service(
        "collector",
        ServiceKind::Background,
        move |ctx| async move {
            let mut units = Units {
                tasks: JoinSet::new(),
                ids: HashMap::new(),
                every: settings.every,
                faults,
                status: status.clone(),
                readings,
            };
            for id in 0..settings.count {
                units.start(id, Duration::ZERO);
            }
            ctx.ready();
            let mut stopped = pin!(ctx.stopped());
            loop {
                if units.tasks.is_empty() {
                    return Error::e_explain(UNITS, "every unit has been given up");
                }
                let ended = tokio::select! {
                    () = &mut stopped => return Ok(()),
                    Some(ended) = units.tasks.join_next_with_id() => ended,
                };
                let (task, why) = match ended {
                    Ok((task, Err(error))) => (task, format!("{error:#}")),
                    Ok((task, Ok(()))) => (task, "the unit ended".to_string()),
                    Err(error) if error.is_panic() => {
                        let task = error.id();
                        let payload = error.into_panic();
                        let text = (payload.downcast_ref::<&str>().map(|text| text.to_string()))
                            .or_else(|| payload.downcast_ref::<String>().cloned());
                        (task, format!("panic: {}", text.unwrap_or_default()))
                    }
                    Err(error) => (error.id(), error.to_string()),
                };
                let Some(id) = units.ids.remove(&task) else {
                    continue;
                };
                let (failures, streak) = status.update(id, |unit| {
                    unit.failures += 1;
                    unit.streak += 1;
                    (unit.failures, unit.streak)
                });
                if streak >= settings.give_up_after {
                    status.update(id, |unit| unit.state = "given up".to_string());
                    tracing::error!(
                        unit = id,
                        failures,
                        why,
                        "a unit has failed {streak} times in a row: given up"
                    );
                } else {
                    status.update(id, |unit| unit.state = "restarting".to_string());
                    tracing::warn!(unit = id, failures, why, "a unit failed: it starts again");
                    units.start(id, settings.every);
                }
            }
        },
    )
}

/// The running units.
struct Units {
    tasks: JoinSet<Result<()>>,
    ids: HashMap<Id, usize>,
    every: Duration,
    faults: Faults,
    status: Status,
    readings: mpsc::Sender<Reading>,
}

impl Units {
    /// Starts unit `id` after `delay`.
    fn start(&mut self, id: usize, delay: Duration) {
        let (every, faults) = (self.every, self.faults.clone());
        let (status, readings) = (self.status.clone(), self.readings.clone());
        let unit = async move {
            tokio::time::sleep(delay).await;
            status.update(id, |unit| unit.state = "running".to_string());
            poll(id, every, &faults, &status, &readings).await
        };
        let task = self
            .tasks
            .spawn(unit.instrument(tracing::info_span!("unit", unit = id)));
        self.ids.insert(task.id(), id);
    }
}

/// A unit: polls its device every `every` and passes on the readings.
async fn poll(
    id: usize,
    every: Duration,
    faults: &Faults,
    status: &Status,
    readings: &mpsc::Sender<Reading>,
) -> Result<()> {
    let mut ticks = tokio::time::interval(every);
    for n in 1_u64.. {
        ticks.tick().await;
        if faults.panic_every > 0 && n % faults.panic_every == 0 {
            panic!("a panic injected in unit {id}, poll {n}");
        }
        if faults.error_every > 0 && n % faults.error_every == 0 {
            return Error::e_explain(DEVICE, format!("an error injected in unit {id}, poll {n}"));
        }
        let value = 200 + 10 * id as i64 + (n % 7) as i64;
        status.update(id, |unit| {
            unit.polls += 1;
            unit.streak = 0;
            unit.last = Some(value);
        });
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |at| at.as_millis() as u64);
        // As the run stops, the store may be gone before the unit.
        let _ = readings
            .send(Reading {
                unit: id,
                value,
                at_ms,
            })
            .await;
    }
    Ok(())
}
