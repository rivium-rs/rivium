//! The tasks of a run: each service's run future and the tasks it spawns, tracked from start to
//! end. A task's end reaches the machine before the task leaves the list, and a service is
//! drained when its last task leaves it. When the run ends, or its future is dropped, every task
//! still running is aborted; blocking tasks cannot be, and keep running.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tracing::Span;

use super::machine::{Exit, Input, panicked};
use crate::log::payload_text;
use crate::{BoxFuture, Result};

/// What the services of a run share with its driver.
#[derive(Debug)]
pub(super) struct Run {
    pub(super) inputs: mpsc::UnboundedSender<Input>,
    /// The deadline, once the run is stopping.
    pub(super) deadline: watch::Sender<Option<Instant>>,
    tasks: Mutex<Tasks>,
}

#[derive(Debug, Default)]
struct Tasks {
    next: u64,
    live: BTreeMap<u64, Task>,
    /// The run is over: no task starts any more.
    closed: bool,
}

#[derive(Debug)]
struct Task {
    service: usize,
    /// `<service>` or `<service>/<task>`.
    path: String,
    /// `None` for a blocking task, which cannot be aborted.
    abort: Option<AbortHandle>,
}

impl Run {
    pub(super) fn new(inputs: mpsc::UnboundedSender<Input>) -> Self {
        Run {
            inputs,
            deadline: watch::Sender::new(None),
            tasks: Mutex::default(),
        }
    }

    /// Starts an async task of service `service`: the service's run future when `task` is
    /// `None`.
    pub(super) fn spawn(
        self: &Arc<Self>,
        service: usize,
        path: String,
        task: Option<Cow<'static, str>>,
        future: BoxFuture<'static, Result<()>>,
    ) {
        // Locked while spawning, so the task cannot leave the list before it enters it.
        let mut tasks = self.tasks();
        if tasks.closed {
            return;
        }
        let (id, run) = (tasks.next, Arc::clone(self));
        tasks.next += 1;
        let handle = tokio::spawn(async move {
            let result = CatchUnwind(future).await;
            run.ended(id, service, task, result);
        });
        let abort = Some(handle.abort_handle());
        tasks.live.insert(
            id,
            Task {
                service,
                path,
                abort,
            },
        );
    }

    /// Starts a blocking task of service `service` on the runtime's blocking threads.
    pub(super) fn spawn_blocking<F>(
        self: &Arc<Self>,
        service: usize,
        path: String,
        (task, span): (Cow<'static, str>, Span),
        f: F,
    ) where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let mut tasks = self.tasks();
        if tasks.closed {
            return;
        }
        let (id, run) = (tasks.next, Arc::clone(self));
        tasks.next += 1;
        tasks.live.insert(
            id,
            Task {
                service,
                path,
                abort: None,
            },
        );
        drop(tasks);
        tokio::task::spawn_blocking(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| span.in_scope(f)));
            let result = result.map_err(|payload| payload_text(&*payload).to_string());
            run.ended(id, service, Some(task), result);
        });
    }

    /// A task ended: its end goes to the machine, then, when it was the last task of its
    /// service, that the service is drained.
    fn ended(
        &self,
        id: u64,
        service: usize,
        task: Option<Cow<'static, str>>,
        result: std::result::Result<Result<()>, String>,
    ) {
        let input = match (task, result) {
            (None, Ok(Ok(()))) => Some(Input::Ended(service, Exit::Returned)),
            (None, Ok(Err(error))) => Some(Input::Ended(service, Exit::Failed(error))),
            (None, Err(message)) => Some(Input::Ended(service, Exit::Panicked(message))),
            (Some(_), Ok(Ok(()))) => None,
            (Some(task), Ok(Err(error))) => Some(Input::ChildFailed(service, task, error)),
            (Some(task), Err(message)) => {
                Some(Input::ChildFailed(service, task, panicked(&message)))
            }
        };
        // Sending fails only once the run is over, when nobody needs to know.
        if let Some(input) = input {
            let _ = self.inputs.send(input);
        }
        let mut tasks = self.tasks();
        tasks.live.remove(&id);
        if !tasks.live.values().any(|task| task.service == service) {
            let _ = self.inputs.send(Input::Drained(service));
        }
    }

    /// Ends the run: no task starts any more, and the tasks still running are aborted. Returns
    /// them, and among them the blocking tasks, which keep running.
    pub(super) fn close(&self) -> (Vec<String>, Vec<String>) {
        let mut tasks = self.tasks();
        tasks.closed = true;
        let (mut abandoned, mut blocking) = (Vec::new(), Vec::new());
        for task in std::mem::take(&mut tasks.live).into_values() {
            match task.abort {
                Some(abort) => abort.abort(),
                None => blocking.push(task.path.clone()),
            }
            abandoned.push(task.path);
        }
        (abandoned, blocking)
    }

    fn tasks(&self) -> MutexGuard<'_, Tasks> {
        // Every change to the list completes under the lock, so a poisoned list is still whole.
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A future whose panic while it is polled becomes its output: the panic's message.
pub(crate) struct CatchUnwind<F>(pub(crate) F);

impl<F: Future + Unpin> Future for CatchUnwind<F> {
    type Output = std::result::Result<F::Output, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.0;
        match panic::catch_unwind(AssertUnwindSafe(|| Pin::new(inner).poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(payload) => Poll::Ready(Err(payload_text(&*payload).to_string())),
        }
    }
}
