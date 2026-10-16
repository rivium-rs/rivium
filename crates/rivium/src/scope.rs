//! Task admission and proof of execution-resource release.

#[cfg(rivium_scope_model)]
use crate::model_support::{Arc, CancellationToken, Mutex};
#[cfg(not(rivium_scope_model))]
use std::sync::{Arc, Mutex};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::Notify;
#[cfg(not(rivium_scope_model))]
use tokio_util::sync::CancellationToken;

/// Whether all accepted execution bodies have released their captures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Drain {
    /// Admission is closed and no execution body remains.
    Complete,
    /// Admission is still open or accepted work remains.
    Incomplete {
        /// Accepted execution bodies or unused reservations still owned.
        active_tasks: usize,
    },
}

impl Drain {
    /// True only when closure and resource release were both confirmed.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

#[derive(Default)]
struct Counts {
    closed: bool,
    active: usize,
}

struct Inner {
    counts: Mutex<Counts>,
    changed: Arc<Notify>,
    cancel: CancellationToken,
}

/// A cloneable owner of task admission, cancellation, and resource accounting.
#[derive(Clone)]
pub struct TaskScope {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TaskScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskScope")
            .field("active_tasks", &self.active_tasks())
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Default for TaskScope {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScope {
    /// Create an open, empty scope.
    pub fn new() -> Self {
        Self::with_notify(Arc::new(Notify::new()))
    }

    pub(crate) fn with_notify(changed: Arc<Notify>) -> Self {
        Self {
            inner: Arc::new(Inner {
                counts: Mutex::new(Counts::default()),
                changed,
                cancel: CancellationToken::new(),
            }),
        }
    }

    /// Reserve all `count` entries atomically, or reject after closure/cancellation.
    /// A zero-sized reservation is still rejected by a closed scope.
    pub fn reserve(&self, count: usize) -> Option<Vec<Reservation>> {
        let mut reservations = Vec::new();
        reservations.try_reserve_exact(count).ok()?;
        let mut counts = self.inner.counts.lock().unwrap_or_else(|p| p.into_inner());
        if counts.closed || self.inner.cancel.is_cancelled() {
            return None;
        }
        counts.active = counts.active.checked_add(count)?;
        drop(counts);
        reservations.extend((0..count).map(|_| Reservation {
            scope: self.clone(),
        }));
        Some(reservations)
    }

    /// Permanently close admission without cancelling accepted work.
    pub fn close(&self) {
        self.inner
            .counts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed = true;
        self.inner.changed.notify_waiters();
    }

    /// Permanently close admission and request cooperative cancellation.
    pub fn cancel(&self) {
        self.close();
        self.inner.cancel.cancel();
    }

    /// Cancellation token shared with accepted work.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.cancel.clone()
    }

    /// Whether new reservations are permanently rejected.
    pub fn is_closed(&self) -> bool {
        self.inner
            .counts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed
            || self.inner.cancel.is_cancelled()
    }

    /// Number of accepted execution bodies plus unused reservations.
    pub fn active_tasks(&self) -> usize {
        self.inner
            .counts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
    }

    /// Current resource-release state, without waiting.
    pub fn state(&self) -> Drain {
        let counts = self.inner.counts.lock().unwrap_or_else(|p| p.into_inner());
        if (counts.closed || self.inner.cancel.is_cancelled()) && counts.active == 0 {
            Drain::Complete
        } else {
            Drain::Incomplete {
                active_tasks: counts.active,
            }
        }
    }

    /// Wait for both admission closure and execution-resource release.
    pub async fn wait(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state().is_complete() {
                return;
            }
            if self.inner.cancel.is_cancelled() {
                notified.await;
            } else {
                tokio::select! {
                    _ = notified => {},
                    _ = self.inner.cancel.cancelled() => {}
                }
            }
        }
    }

    /// Observe release within one total wait budget; this does not request stop.
    pub async fn drain(&self, within: Duration) -> Drain {
        if !self.state().is_complete() && !within.is_zero() {
            let _ = tokio::time::timeout(within, self.wait()).await;
        }
        self.state()
    }
}

/// An accepted task slot. Dropping an unused slot releases its accounting.
#[derive(Debug)]
pub struct Reservation {
    scope: TaskScope,
}

impl Reservation {
    /// Keep the reservation until the future and all its captures are dropped.
    pub fn track<F: Future>(self, future: F) -> Tracked<F> {
        Tracked {
            future: Some(Box::pin(future)),
            reservation: Some(self),
        }
    }

    /// Run a blocking closure while retaining its reservation through unwinding.
    pub fn run<T>(self, work: impl FnOnce() -> T) -> T {
        let output = work();
        drop(self);
        output
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        {
            let mut counts = self
                .scope
                .inner
                .counts
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            counts.active -= 1;
        }
        self.scope.inner.changed.notify_waiters();
    }
}

/// Future wrapper that releases the execution body before its reservation.
#[must_use = "futures do nothing unless polled or spawned"]
pub struct Tracked<F: Future> {
    // Field order also preserves the release ordering during unwinding and unpolled drop.
    future: Option<Pin<Box<F>>>,
    reservation: Option<Reservation>,
}

impl<F: Future> Future for Tracked<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = this
            .future
            .as_mut()
            .expect("tracked future polled after completion")
            .as_mut()
            .poll(cx);
        if result.is_ready() {
            drop(this.future.take());
            drop(this.reservation.take());
        }
        result
    }
}
