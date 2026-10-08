//! The log files: one thread writes every file sink from a bounded, lossless queue, rolls the
//! files by date and size, and answers flush barriers; another compresses rolled files.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::budget::{Budget, Kind, scan};

/// Events the queue holds before writers wait.
const QUEUE: usize = 8_192;
/// How often a sink checks that its file still exists, and retries a failed rename.
const RETRY: Duration = Duration::from_secs(1);
/// How often the budget is enforced without a roll or a compression.
const HOURLY: Duration = Duration::from_secs(3_600);

/// The time, for the dates in file names: injected so that tests can move it.
pub(crate) type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// What the log files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilesConfig {
    pub(crate) dir: PathBuf,
    /// The base name of each sink's file; the first is the main file, named after the service.
    pub(crate) sinks: Vec<String>,
    pub(crate) max_file_size: u64,
    pub(crate) max_total_size: u64,
    /// The share of the budget kept for log export files.
    pub(crate) export_reserve: u64,
}

/// Counts for `rivium::stats()`.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) bytes: AtomicU64,
    pub(crate) failures: AtomicU64,
    pub(crate) deleted: AtomicU64,
    /// Whether a failure has been written to stderr: only the first one is.
    reported: AtomicBool,
}

enum Message {
    Line(usize, Vec<u8>),
    Flush(Sender<()>),
}

/// Queues lines for one sink.
#[derive(Clone)]
pub(crate) struct SinkSender {
    queue: SyncSender<Message>,
    sink: usize,
}

impl SinkSender {
    /// Queues one line; waits while the queue is full. `false` when the writer is gone.
    pub(crate) fn send(&self, line: Vec<u8>) -> bool {
        self.queue.send(Message::Line(self.sink, line)).is_ok()
    }
}

/// Running log files. Dropping every sender of the queue stops the threads after they have
/// written and compressed what they hold.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the threads are joined by tests only")
)]
pub(crate) struct Files {
    queue: SyncSender<Message>,
    pub(crate) counters: Arc<Counters>,
    threads: Vec<JoinHandle<()>>,
}

impl Files {
    /// Creates the directory, rolls active files left from an earlier day, removes files left
    /// half written, queues rolled files left uncompressed, enforces the budget and starts the
    /// threads.
    ///
    /// # Errors
    ///
    /// When the directory or an active file cannot be created or opened, or a thread cannot be
    /// started.
    pub(crate) fn start(config: FilesConfig, clock: Clock) -> io::Result<Files> {
        fs::create_dir_all(&config.dir)?;
        let counters = Arc::new(Counters::default());
        let target = (config.max_total_size)
            .saturating_sub(config.max_file_size)
            .saturating_sub(config.export_reserve);
        let budget = Arc::new(Budget {
            dir: config.dir.clone(),
            sinks: config.sinks.clone(),
            target,
            counters: Arc::clone(&counters),
            compressing: Mutex::new(None),
        });
        let today = day(clock());
        let mut leftovers = Vec::new();
        let mut next = 1;
        for entry in scan(&config.dir) {
            if let Kind::Rolled(seq) | Kind::Archive(seq) | Kind::Partial(seq) = entry.kind {
                next = next.max(seq + 1);
            }
            match entry.kind {
                Kind::Partial(_) | Kind::Export => drop(fs::remove_file(&entry.path)),
                Kind::Rolled(seq) => leftovers.push((seq, entry.path)),
                _ => {}
            }
        }
        let mut sinks = Vec::new();
        for base in &config.sinks {
            let path = config.dir.join(format!("{base}.log"));
            let meta = fs::metadata(&path).ok().filter(|meta| meta.len() > 0);
            if let Some(modified) = meta.and_then(|meta| meta.modified().ok()).map(day)
                && modified < today
            {
                // Written on an earlier day: roll it first, unless it is held open elsewhere.
                let rolled = config
                    .dir
                    .join(format!("{base}.{}.{next}.log", date(modified)));
                if fs::rename(&path, &rolled).is_ok() {
                    leftovers.push((next, rolled));
                    next += 1;
                }
            }
            sinks.push(Sink::open(path, today)?);
        }
        leftovers.sort();
        budget.enforce();

        let (rolled, to_compress) = mpsc::channel();
        for (_, path) in leftovers {
            let _ = rolled.send(path);
        }
        let (queue, received) = mpsc::sync_channel(QUEUE);
        let name = config.sinks.first().cloned().unwrap_or_default();
        let compressor = Arc::clone(&budget);
        let threads = vec![
            spawn(format!("{name}-log-gzip"), move || {
                compressor.compress(&to_compress)
            })?,
            spawn(format!("{name}-log"), {
                let counters = Arc::clone(&counters);
                let writer = Writer {
                    config,
                    clock,
                    sinks,
                    next,
                    rolled,
                    budget,
                    counters,
                };
                move || writer.run(&received)
            })?,
        ];
        Ok(Files {
            queue,
            counters,
            threads,
        })
    }

    /// What the output layer of the sink with this index writes to.
    pub(crate) fn sender(&self, sink: usize) -> SinkSender {
        SinkSender {
            queue: self.queue.clone(),
            sink,
        }
    }

    /// Returns once every line queued before has been written to the operating system (not
    /// synced to disk), or `false` when that takes longer than `timeout`.
    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (done, wait) = mpsc::channel();
        let mut message = Message::Flush(done);
        loop {
            match self.queue.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Full(again)) if Instant::now() < deadline => {
                    message = again;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return false,
            }
        }
        wait.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }

    /// Stops the threads once every queued line is written and every rolled file compressed.
    /// Every [`SinkSender`] must be dropped first.
    #[cfg(test)]
    pub(crate) fn stop(self) {
        drop(self.queue);
        for thread in self.threads {
            thread.join().expect("log thread");
        }
    }
}

fn spawn(name: String, run: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name(name).spawn(run)
}

/// The days since the Unix epoch, UTC.
pub(super) fn day(time: SystemTime) -> i64 {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    };
    seconds.div_euclid(86_400)
}

/// A day as `YYYY-MM-DD` (proleptic Gregorian calendar).
pub(super) fn date(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A day as year, month and day of the proleptic Gregorian calendar.
pub(super) fn civil(day: i64) -> (i64, i64, i64) {
    let z = day + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (
        doy - (153 * mp + 2) / 5 + 1,
        if mp < 10 { mp + 3 } else { mp - 9 },
    );
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// One sink's active file.
struct Sink {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    size: u64,
    /// The day of the lines in the file.
    day: i64,
    /// When to check that the file still exists, or to retry a failed roll.
    check_at: Instant,
    roll_at: Instant,
}

impl Sink {
    fn open(path: PathBuf, day: i64) -> io::Result<Sink> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        let now = Instant::now();
        Ok(Sink {
            path,
            file: Some(BufWriter::new(file)),
            size,
            day,
            check_at: now + RETRY,
            roll_at: now,
        })
    }

    /// Opens the file again, making the directory first if it is gone.
    fn reopen(&mut self) -> io::Result<()> {
        self.file = None;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.size = file.metadata()?.len();
        self.file = Some(BufWriter::new(file));
        Ok(())
    }
}

struct Writer {
    config: FilesConfig,
    clock: Clock,
    sinks: Vec<Sink>,
    /// The sequence number of the next rolled file.
    next: u64,
    rolled: Sender<PathBuf>,
    budget: Arc<Budget>,
    counters: Arc<Counters>,
}

impl Writer {
    fn run(mut self, queue: &Receiver<Message>) {
        let mut enforce_at = Instant::now() + HOURLY;
        loop {
            match queue.recv_timeout(enforce_at.saturating_duration_since(Instant::now())) {
                Ok(message) => {
                    self.handle(message);
                    // Everything queued meanwhile is one batch, flushed once.
                    while let Ok(message) = queue.try_recv() {
                        self.handle(message);
                    }
                    self.flush();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if Instant::now() >= enforce_at {
                self.budget.enforce();
                enforce_at = Instant::now() + HOURLY;
            }
        }
        self.flush();
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Line(sink, line) if sink < self.sinks.len() => self.write(sink, line),
            Message::Line(..) => {}
            Message::Flush(done) => {
                self.flush();
                let _ = done.send(());
            }
        }
    }

    fn write(&mut self, index: usize, mut line: Vec<u8>) {
        let max = self.config.max_file_size;
        if line.len() as u64 > max {
            // One event larger than a whole file: keep its start, at a character boundary.
            let mut end = usize::try_from(max).unwrap_or(usize::MAX).saturating_sub(1);
            while end > 0 && line[end] & 0xC0 == 0x80 {
                end -= 1;
            }
            line.truncate(end);
            line.push(b'\n');
        }
        let today = day((self.clock)());
        let now = Instant::now();
        let sink = &mut self.sinks[index];
        if now >= sink.check_at {
            sink.check_at = now + RETRY;
            if sink.file.is_none() || !sink.path.exists() {
                let reopened = sink.reopen();
                self.failed(reopened.err());
            }
        }
        let sink = &mut self.sinks[index];
        if sink.size == 0 {
            // Nothing to roll: the empty file takes the new day.
            sink.day = today;
        }
        let full = sink.size > 0 && sink.size + line.len() as u64 > max;
        if (sink.day != today || full) && now >= sink.roll_at {
            self.roll(index, today);
        }
        if self.sinks[index].file.is_none() {
            return self.failed(Some(io::Error::other("the file is not open")));
        }
        let sink = &mut self.sinks[index];
        let written = sink
            .file
            .as_mut()
            .map_or(Ok(()), |file| file.write_all(&line));
        sink.size += line.len() as u64;
        match written {
            Ok(()) => drop(
                self.counters
                    .bytes
                    .fetch_add(line.len() as u64, Ordering::Relaxed),
            ),
            Err(error) => self.failed(Some(error)),
        }
    }

    /// Closes the active file, renames it to its rolled name, hands it to the compressor and
    /// opens a new one. When the rename fails, as on Windows while another process holds the
    /// file, writing goes on in the active file and the roll is retried a second later.
    fn roll(&mut self, index: usize, today: i64) {
        let sink = &mut self.sinks[index];
        if let Some(mut file) = sink.file.take() {
            let _ = file.flush();
        }
        let base = &self.config.sinks[index];
        let rolled = self
            .config
            .dir
            .join(format!("{base}.{}.{}.log", date(sink.day), self.next));
        let renamed = fs::rename(&sink.path, &rolled);
        let reopened = sink.reopen();
        match renamed {
            Ok(()) => {
                self.next += 1;
                sink.day = today;
                let _ = self.rolled.send(rolled);
                self.budget.enforce();
            }
            Err(error) => {
                sink.roll_at = Instant::now() + RETRY;
                self.failed(Some(error));
            }
        }
        self.failed(reopened.err());
    }

    fn flush(&mut self) {
        for index in 0..self.sinks.len() {
            let flushed = self.sinks[index].file.as_mut().map(BufWriter::flush);
            self.failed(flushed.and_then(Result::err));
        }
    }

    /// Counts a failure; the first one is also written to stderr.
    fn failed(&self, error: Option<io::Error>) {
        let Some(error) = error else { return };
        self.counters.failures.fetch_add(1, Ordering::Relaxed);
        if !self.counters.reported.swap(true, Ordering::Relaxed) {
            let (name, dir) = (&self.config.sinks[0], self.config.dir.display());
            eprintln!(
                "{name}: cannot write the log files in {dir}: {error}; further failures are only counted"
            );
        }
    }
}

#[cfg(test)]
mod tests;
