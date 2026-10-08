//! Scripted services: services that follow a list of steps, to drive the supervisor through
//! any order of events.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rivium::error::{Class, ErrorKind, ErrorType};
use rivium::{BoxFuture, Error, Result, Service, ServiceContext, ServiceKind};

/// The kind of the errors that [`Step::Fail`] returns.
pub const SCRIPTED: ErrorType = &ErrorKind::new("Scripted", Class::Internal);

/// One step of a [`ScriptedService`].
#[derive(Clone, Debug)]
pub enum Step {
    /// Calls `ready()`.
    Ready,
    /// Sleeps this long, whether or not the service is asked to stop meanwhile.
    Sleep(Duration),
    /// Waits until the service is asked to stop.
    UntilStopped,
    /// Returns `Ok(())`.
    Return,
    /// Returns a [`SCRIPTED`] error with this context.
    Fail(&'static str),
    /// Panics with this message.
    Panic(&'static str),
    /// Waits until the service is asked to stop, then keeps running this long: a slow stop.
    IgnoreStop(Duration),
    /// Starts a blocking task named `block` that holds its thread this long, and goes on to the
    /// next step at once.
    Block(Duration),
}

/// A service that runs its steps in order and returns `Ok(())` after the last one.
///
/// ```
/// use std::time::Duration;
///
/// use rivium::ServiceKind;
/// use rivium_test::{ScriptedService, Step};
///
/// // A server that is ready after a second and needs two seconds to stop.
/// let slow = ScriptedService::new(
///     "http",
///     ServiceKind::Frontline,
///     vec![Step::Sleep(Duration::from_secs(1)), Step::Ready, Step::IgnoreStop(Duration::from_secs(2))],
/// );
/// let journal = slow.journal();
/// # drop(slow);
/// assert!(journal.entries().is_empty());
/// ```
#[derive(Clone, Debug)]
pub struct ScriptedService {
    name: Cow<'static, str>,
    kind: ServiceKind,
    steps: Vec<Step>,
    journal: Journal,
}

impl ScriptedService {
    /// A service with a name, a kind and its steps.
    #[must_use]
    pub fn new(name: impl Into<Cow<'static, str>>, kind: ServiceKind, steps: Vec<Step>) -> Self {
        ScriptedService {
            name: name.into(),
            kind,
            steps,
            journal: Journal::default(),
        }
    }

    /// What the service has done so far; clones share it.
    #[must_use]
    pub fn journal(&self) -> Journal {
        self.journal.clone()
    }
}

impl Service for ScriptedService {
    fn name(&self) -> Cow<'static, str> {
        self.name.clone()
    }

    fn kind(&self) -> ServiceKind {
        self.kind
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let journal = self.journal;
            journal.push("started");
            for step in self.steps {
                match step {
                    Step::Ready => {
                        ctx.ready();
                        journal.push("ready");
                    }
                    Step::Sleep(duration) => tokio::time::sleep(duration).await,
                    Step::UntilStopped => {
                        ctx.stopped().await;
                        journal.push("asked to stop");
                    }
                    Step::Return => break,
                    Step::Fail(context) => {
                        journal.push("failed");
                        return Error::e_explain(SCRIPTED, context);
                    }
                    Step::Panic(message) => {
                        journal.push("panicking");
                        // Unwinds like a panic, without the panic hook's message.
                        std::panic::resume_unwind(Box::new(message));
                    }
                    Step::IgnoreStop(duration) => {
                        ctx.stopped().await;
                        journal.push("asked to stop");
                        tokio::time::sleep(duration).await;
                    }
                    Step::Block(duration) => ctx.spawn_blocking("block", move || {
                        std::thread::sleep(duration);
                        Ok(())
                    }),
                }
            }
            journal.push("returned");
            Ok(())
        })
    }
}

/// What a [`ScriptedService`] did, in order: `started`, `ready`, `asked to stop`, then
/// `returned`, `failed` or `panicking`.
#[derive(Clone, Debug, Default)]
pub struct Journal(Arc<Mutex<Vec<&'static str>>>);

impl Journal {
    fn push(&self, entry: &'static str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    /// The entries so far.
    #[must_use]
    pub fn entries(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
