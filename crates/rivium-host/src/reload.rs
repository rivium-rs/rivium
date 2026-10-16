use crate::{
    App, Options,
    config::{self, Store},
    session,
};
use rivium::{Error, Result, error::kinds};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Instant,
};
use tokio::sync::{oneshot, watch};

struct Shared {
    gate: Arc<Mutex<bool>>,
    closed: watch::Sender<bool>,
    done: Mutex<bool>,
    failure: Mutex<Option<Error>>,
    finished: Condvar,
}
enum Command {
    Reload(oneshot::Sender<Result<config::ReloadReport>>),
    Close,
}
#[derive(Clone)]
pub(crate) struct Requester {
    sender: mpsc::SyncSender<Command>,
    shared: Arc<Shared>,
}
impl Requester {
    pub(crate) fn start<A: App>(
        options: Options,
        config: A::Config,
        baseline: toml::Value,
        dispatch: tracing::Dispatch,
        gate: Arc<Mutex<bool>>,
    ) -> Result<(Self, config::Handle<A::Config>)> {
        let store = Store::new(config);
        let handle = store.handle();
        let (sender, receiver) = mpsc::sync_channel(8);
        let (closed, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            gate,
            closed,
            done: Mutex::new(false),
            failure: Mutex::new(None),
            finished: Condvar::new(),
        });
        let owner = shared.clone();
        std::thread::Builder::new()
            .name(format!("{}-config", A::NAME))
            .spawn(move || {
                struct Finished(Arc<Shared>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        *crate::lock(&self.0.done) = true;
                        self.0.finished.notify_all();
                    }
                }
                let _finished = Finished(owner.clone());
                let completed = catch_unwind(AssertUnwindSafe(
                    || tracing::dispatcher::with_default(&dispatch, || {
                        let mut store = store;
                        let mut baseline = baseline;
                        while let Ok(command) = receiver.recv() {
                            let Command::Reload(reply) = command else {
                                break;
                            };
                            if !*crate::lock(&owner.gate) {
                                let _ = reply.send(Err(unavailable()));
                                break;
                            }
                            if reply.is_closed() {
                                continue;
                            }
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                let candidate = session::load::<A>(&options, None, true)?;
                                let current = store.handle().snapshot();
                                let effective = A::reload_config(&current.value, &candidate.config)?;
                                if let Some(value) = &effective {
                                    A::validate(value)?;
                                }
                                let mut next = baseline.clone();
                                if let Some(value) = &effective {
                                    session::replace_application(&mut next, value)?;
                                }
                                let published = session::differences(&baseline, &next);
                                let requires_restart = session::differences(&next, &candidate.all);
                                let guard = crate::lock(&owner.gate);
                                if !*guard || reply.is_closed() {
                                    drop(guard);
                                    return Err(unavailable());
                                }
                                let generation = if published.is_empty() {
                                    drop(guard);
                                    current.generation
                                } else {
                                    let value = effective
                                        .expect("only an effective application change can publish");
                                    let generation = store.publish_guarded(value, guard)?;
                                    baseline = next;
                                    generation
                                };
                                Ok(config::ReloadReport {
                                    generation,
                                    published,
                                    requires_restart,
                                })
                            }))
                            .unwrap_or_else(|_| {
                                Err(Error::internal("configuration reload preparation panicked"))
                            });
                            if let Err(error) = &result {
                                tracing::warn!(class = ?error.class(), "configuration reload rejected");
                            }
                            let _ = reply.send(result);
                            if !*crate::lock(&owner.gate) {
                                break;
                            }
                        }
                    })
                ));
                if completed.is_err() {
                    *crate::lock(&owner.failure) =
                        Some(Error::internal("configuration worker panicked"));
                    *crate::lock(&owner.gate) = false;
                    owner.closed.send_replace(true);
                }
            })
            .map_err(Error::from)?;
        Ok((Self { sender, shared }, handle))
    }
    pub(crate) async fn request(&self) -> Result<config::ReloadReport> {
        let mut closed = self.shared.closed.subscribe();
        let (reply, result) = oneshot::channel();
        let submitted = {
            let guard = crate::lock(&self.shared.gate);
            if !*guard {
                return Err(unavailable());
            }
            self.sender.try_send(Command::Reload(reply))
        };
        submitted.map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                Error::new(kinds::CONFLICT, "configuration reload queue is full")
            }
            mpsc::TrySendError::Disconnected(_) => unavailable(),
        })?;
        tokio::select! {
            biased;
            _ = async {
                if !*closed.borrow() {
                    let _ = closed.changed().await;
                }
            } => Err(unavailable()),
            result = result => result.unwrap_or_else(|_| Err(unavailable())),
        }
    }
    pub(crate) fn close(&self) {
        *crate::lock(&self.shared.gate) = false;
        let notified = *self.shared.closed.borrow();
        if !notified {
            self.shared.closed.send_replace(true);
            let _ = self.sender.try_send(Command::Close);
        }
    }
    pub(crate) fn failure(&self) -> Option<Error> {
        crate::lock(&self.shared.failure).clone()
    }
    pub(crate) fn pending(&self) -> usize {
        usize::from(!*crate::lock(&self.shared.done))
    }
    pub(crate) fn wait(&self, deadline: Instant) {
        let mut done = crate::lock(&self.shared.done);
        while !*done {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            done = self
                .shared
                .finished
                .wait_timeout(done, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}
fn unavailable() -> Error {
    Error::new(kinds::UNAVAILABLE, "configuration generation is closed")
}
