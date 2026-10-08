//! Log capture: the events a test logs, as JSON objects.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value};
use tracing::Dispatch;
use tracing::subscriber::{DefaultGuard, NoSubscriber};

/// Captures every event logged on this thread, at every level, until the capture is dropped.
/// It sets the thread's default subscriber and installs nothing globally, so tests that
/// capture run in parallel. The code under test must log on this thread, as it does on a
/// current-thread runtime (the default of `#[tokio::test]`).
///
/// ```
/// let logs = rivium_test::capture_logs();
/// tracing::warn!(service.name = "poller", "slow");
/// let events = logs.with_message("slow");
/// assert_eq!(events[0]["service.name"], "poller");
/// assert_eq!(events[0]["level"], "WARN");
/// ```
#[must_use]
pub fn capture_logs() -> LogCapture {
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    // While only one subscriber exists, a callsite that another thread reaches first, with no
    // subscriber of its own, is cached as never enabled, and this capture would miss its
    // events. With a second one, tracing asks every live subscriber instead.
    let second = Dispatch::new(NoSubscriber::default());
    let guard = tracing::subscriber::set_default(subscriber);
    LogCapture {
        buffer,
        _guard: guard,
        _second: second,
    }
}

/// The events captured by [`capture_logs`].
#[derive(Debug)]
pub struct LogCapture {
    buffer: Buffer,
    _guard: DefaultGuard,
    _second: Dispatch,
}

impl LogCapture {
    /// Every event so far: its fields at the top, with `message`, `level` and `target`, and
    /// its spans under `spans`.
    #[must_use]
    pub fn events(&self) -> Vec<Map<String, Value>> {
        let bytes = self
            .buffer
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The events with this message.
    #[must_use]
    pub fn with_message(&self, message: &str) -> Vec<Map<String, Value>> {
        (self.events().into_iter())
            .filter(|event| event.get("message").and_then(Value::as_str) == Some(message))
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Appending cannot leave the buffer half changed: a poisoned lock still holds lines.
        let mut buffer = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::capture_logs;

    #[test]
    fn a_capture_sees_callsites_that_another_thread_reached_first() {
        fn log() {
            tracing::warn!("from a shared callsite");
        }
        let logs = capture_logs();
        // A thread without a subscriber of its own reaches the callsite first.
        std::thread::spawn(log).join().unwrap();
        log();
        assert_eq!(logs.with_message("from a shared callsite").len(), 1);
    }
}
