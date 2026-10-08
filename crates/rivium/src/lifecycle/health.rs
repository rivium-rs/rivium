//! Readiness: the phase of the run and the health items its services report. Services push the
//! state of an item when it changes, so reading readiness does no I/O and calls no service code.

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::watch;

/// The lifecycle phase of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// The services are starting.
    Starting,
    /// Every service is ready.
    Running,
    /// The services are asked to stop: frontline services first, then background services.
    Stopping,
    /// The run is over.
    Stopped,
}

impl Phase {
    /// The phase in lower case, as logged.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Stopping => "stopping",
            Phase::Stopped => "stopped",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The state of a health item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// Working.
    Healthy,
    /// Not working, with a short reason for operators.
    Unhealthy(Cow<'static, str>),
}

#[derive(Debug)]
struct Item {
    name: Cow<'static, str>,
    health: Mutex<Health>,
}

/// The health items of a run, such as a database connection a service depends on. Clones share
/// the items.
#[derive(Clone, Debug, Default)]
pub struct HealthRegistry {
    items: Arc<Mutex<Vec<Weak<Item>>>>,
}

impl HealthRegistry {
    /// Adds an item, healthy at first. It is reported until the last clone of its handle is
    /// dropped.
    pub fn register(&self, name: impl Into<Cow<'static, str>>) -> HealthHandle {
        let item = Arc::new(Item {
            name: name.into(),
            health: Mutex::new(Health::Healthy),
        });
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.retain(|item| item.strong_count() > 0);
        items.push(Arc::downgrade(&item));
        HealthHandle(item)
    }

    /// Each item's name and state, in the order they were registered.
    fn checks(&self) -> Vec<(Cow<'static, str>, Health)> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        (items.iter().filter_map(Weak::upgrade))
            .map(|item| (item.name.clone(), item.health()))
            .collect()
    }
}

/// Sets the state of one health item.
#[derive(Clone, Debug)]
pub struct HealthHandle(Arc<Item>);

impl HealthHandle {
    /// Sets the item's state.
    pub fn set(&self, health: Health) {
        *self.0.health.lock().unwrap_or_else(PoisonError::into_inner) = health;
    }
}

impl Item {
    fn health(&self) -> Health {
        self.health
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Whether the service should get traffic: the run is [`Phase::Running`] and every health item
/// is [`Health::Healthy`]. Probes read it; clones see the same run.
#[derive(Clone, Debug)]
pub struct Readiness {
    pub(crate) phase: watch::Receiver<Phase>,
    health: HealthRegistry,
}

impl Readiness {
    pub(crate) fn new(phase: watch::Receiver<Phase>, health: HealthRegistry) -> Self {
        Readiness { phase, health }
    }

    /// The phase of the run.
    #[must_use]
    pub fn phase(&self) -> Phase {
        *self.phase.borrow()
    }

    /// Each health item's name and state, in the order they were registered.
    #[must_use]
    pub fn checks(&self) -> Vec<(Cow<'static, str>, Health)> {
        self.health.checks()
    }

    /// Whether the service is ready: running, with every health item healthy.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.phase() == Phase::Running
            && (self.checks().iter()).all(|(_, health)| *health == Health::Healthy)
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::watch;

    use super::{Health, HealthRegistry, Phase, Readiness};

    #[test]
    fn ready_means_running_with_every_item_healthy() {
        let (phase, phases) = watch::channel(Phase::Starting);
        let registry = HealthRegistry::default();
        let readiness = Readiness::new(phases, registry.clone());
        let store = registry.register("store");
        assert!(!readiness.is_ready(), "starting");
        phase.send_replace(Phase::Running);
        assert!(readiness.is_ready());
        store.set(Health::Unhealthy("disk full".into()));
        let queue = registry.register("queue");
        assert_eq!(
            readiness.checks(),
            [
                ("store".into(), Health::Unhealthy("disk full".into())),
                ("queue".into(), Health::Healthy)
            ]
        );
        assert!(!readiness.is_ready());
        // An item goes away with its last handle.
        let clone = store.clone();
        drop(store);
        assert!(!readiness.is_ready());
        drop(clone);
        assert_eq!(readiness.checks(), [("queue".into(), Health::Healthy)]);
        assert!(readiness.is_ready());
        drop(queue);
        phase.send_replace(Phase::Stopping);
        assert!(!readiness.is_ready(), "stopping");
    }
}
