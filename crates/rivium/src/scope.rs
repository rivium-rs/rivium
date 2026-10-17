//! Task admission and proof of execution-resource release.

#[cfg(rivium_scope_model)]
use crate::model_support::{Arc, AtomicUsize, CancellationToken, Mutex, Ordering};
#[cfg(not(rivium_scope_model))]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
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

struct Inner {
    closed: Mutex<bool>,
    // High bits count reservations; the low bit permanently enables release notifications.
    state: AtomicUsize,
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
        Self::create(Arc::new(Notify::new()), false)
    }

    pub(crate) fn with_notify(changed: Arc<Notify>) -> Self {
        Self::create(changed, true)
    }

    fn create(changed: Arc<Notify>, observed: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                closed: Mutex::new(false),
                state: AtomicUsize::new(usize::from(observed)),
                changed,
                cancel: CancellationToken::new(),
            }),
        }
    }

    /// Reserve all `count` entries atomically, or reject after closure/cancellation.
    /// A zero-sized reservation is still rejected by a closed scope.
    #[inline]
    pub fn reserve(&self, count: usize) -> Option<Vec<Reservation>> {
        let closed = self.inner.closed.lock().unwrap_or_else(|p| p.into_inner());
        if *closed || self.inner.cancel.is_cancelled() {
            return None;
        }
        let mut reservations = Vec::new();
        reservations.try_reserve_exact(count).ok()?;
        if count > 0 {
            let increment = count.checked_mul(2)?;
            let active = self.inner.state.load(Ordering::Relaxed) >> 1;
            active.checked_add(count)?.checked_mul(2)?;
            // Admission holds the only increment lock; concurrent releases only lower this bound.
            self.inner.state.fetch_add(increment, Ordering::Relaxed);
        }
        drop(closed);
        reservations.extend((0..count).map(|_| Reservation {
            scope: self.clone(),
        }));
        Some(reservations)
    }

    /// Permanently close admission without cancelling accepted work.
    pub fn close(&self) {
        *self.inner.closed.lock().unwrap_or_else(|p| p.into_inner()) = true;
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
        *self.inner.closed.lock().unwrap_or_else(|p| p.into_inner())
            || self.inner.cancel.is_cancelled()
    }

    /// Number of accepted execution bodies plus unused reservations.
    pub fn active_tasks(&self) -> usize {
        self.inner.state.load(Ordering::Acquire) >> 1
    }

    /// Current resource-release state, without waiting.
    pub fn state(&self) -> Drain {
        let closed = self.inner.closed.lock().unwrap_or_else(|p| p.into_inner());
        let stopped = *closed || self.inner.cancel.is_cancelled();
        let active = self.active_tasks();
        if stopped && active == 0 {
            Drain::Complete
        } else {
            Drain::Incomplete {
                active_tasks: active,
            }
        }
    }

    /// Wait for both admission closure and execution-resource release.
    pub async fn wait(&self) {
        // Share the counter's modification order so enrollment cannot miss the final release.
        self.inner.state.fetch_or(1, Ordering::AcqRel);
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
    #[inline]
    fn drop(&mut self) {
        // Publish capture release before an observer can confirm zero active tasks.
        if self.scope.inner.state.fetch_sub(2, Ordering::Release) == 3 {
            // Acquire the release sequence from all bodies before invoking their waiters.
            let _ = self.scope.inner.state.load(Ordering::Acquire);
            // The shared supervisor notification also observes open scopes becoming empty.
            self.scope.inner.changed.notify_waiters();
        }
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
