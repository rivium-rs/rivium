use super::{FileSettings, LogStats};
use rivium::{Error, Result};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_OWNED_FILES: usize = 8192;

static WRITTEN: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);
static DELETED: AtomicU64 = AtomicU64::new(0);

pub(super) fn counters() -> [u64; 3] {
    [
        WRITTEN.load(Ordering::Relaxed),
        FAILURES.load(Ordering::Relaxed),
        DELETED.load(Ordering::Relaxed),
    ]
}

pub(super) struct Directory {
    pub(super) path: PathBuf,
    pub(super) io: Mutex<()>,
    _lock: File,
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Arc<Self>> {
        fs::create_dir_all(path).map_err(io_error)?;
        let path = path.canonicalize().map_err(io_error)?;
        let lock_path = path.join(".rivium.lock");
        reject_symlink(&lock_path).map_err(io_error)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .map_err(io_error)?;
        lock.try_lock().map_err(|e| {
            Error::new(
                rivium::error::kinds::CONFLICT,
                format!("log directory already owned: {e}"),
            )
        })?;
        Ok(Arc::new(Self {
            path,
            io: Mutex::new(()),
            _lock: lock,
        }))
    }
}

pub(super) fn io_error(e: io::Error) -> Error {
    Error::internal(format!("log IO: {e}")).with_source(e)
}

fn failure(message: &str) -> Error {
    Error::internal(message.to_owned())
}

pub(super) fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_file() => {
            Err(io::Error::other("log path is not a regular file"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

struct Event {
    sink: usize,
    data: Vec<u8>,
}

struct Queue {
    events: VecDeque<Event>,
    bytes: usize,
    accepted: u64,
    completed: u64,
    flush_requested: u64,
    flush_serial: u64,
    flush_done: u64,
    closed: bool,
    done: bool,
    failure: Option<String>,
}

struct Control {
    #[cfg(all(test, feature = "log-export"))]
    release_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    _thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    queue: Mutex<Queue>,
    changed: Condvar,
    dropped: AtomicU64,
    oversized: AtomicU64,
    failures: AtomicU64,
}

#[derive(Clone)]
pub(super) struct Worker(Arc<Control>);

impl Worker {
    pub(super) fn new(
        directory: Arc<Directory>,
        names: Vec<String>,
        settings: &FileSettings,
        reserve: u64,
    ) -> Result<Self> {
        let mut files = Files::new(&directory, names, settings, reserve).map_err(io_error)?;
        let control = Arc::new(Control {
            #[cfg(all(test, feature = "log-export"))]
            release_gate: Mutex::new(None),
            _thread: Mutex::new(None),
            queue: Mutex::new(Queue {
                events: VecDeque::new(),
                bytes: 0,
                accepted: 0,
                completed: 0,
                flush_requested: 0,
                flush_serial: 0,
                flush_done: 0,
                closed: false,
                done: false,
                failure: None,
            }),
            changed: Condvar::new(),
            dropped: AtomicU64::new(0),
            oversized: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        });
        let worker = Self(Arc::clone(&control));
        let thread = std::thread::Builder::new()
            .name("rivium-log".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    loop {
                        let (event, flush, serial, closing) = {
                            let mut queue = super::lock(&control.queue);
                            while queue.events.is_empty()
                                && !queue.closed
                                && queue.flush_done == queue.flush_serial
                            {
                                queue = control
                                    .changed
                                    .wait(queue)
                                    .unwrap_or_else(|e| e.into_inner());
                            }
                            let event = queue.events.pop_front();
                            if let Some(event) = &event {
                                queue.bytes -= event.data.len();
                            }
                            let flush = (queue.flush_done < queue.flush_serial
                                && queue.completed >= queue.flush_requested)
                                || (event.is_none() && queue.closed);
                            (event, flush, queue.flush_serial, queue.closed)
                        };
                        if let Some(event) = event {
                            let result = {
                                let _guard = super::lock(&directory.io);
                                files.write(&directory.path, event)
                            };
                            let mut queue = super::lock(&control.queue);
                            queue.completed += 1;
                            if let Err(error) = result {
                                control.failures.fetch_add(1, Ordering::Relaxed);
                                FAILURES.fetch_add(1, Ordering::Relaxed);
                                queue
                                    .failure
                                    .get_or_insert_with(|| format!("log write failed: {error}"));
                                files.failed = true;
                            }
                        }
                        if flush {
                            #[cfg(all(test, feature = "log-export"))]
                            if closing
                                && let Some(gate) = control.release_gate.lock().unwrap().take()
                            {
                                let _ = gate.recv();
                            }
                            let result = {
                                let _guard = super::lock(&directory.io);
                                files.flush(&directory.path)
                            };
                            let mut queue = super::lock(&control.queue);
                            if let Err(error) = result {
                                control.failures.fetch_add(1, Ordering::Relaxed);
                                FAILURES.fetch_add(1, Ordering::Relaxed);
                                queue
                                    .failure
                                    .get_or_insert_with(|| format!("log flush failed: {error}"));
                                files.failed = true;
                            }
                            queue.flush_done = serial;
                            if closing && queue.events.is_empty() {
                                break;
                            }
                            drop(queue);
                            control.changed.notify_all();
                        }
                    }
                }));
                if outcome.is_err() {
                    control.failures.fetch_add(1, Ordering::Relaxed);
                    FAILURES.fetch_add(1, Ordering::Relaxed);
                    let mut queue = super::lock(&control.queue);
                    queue.closed = true;
                    queue
                        .failure
                        .get_or_insert_with(|| "log writer panicked".into());
                    queue.events.clear();
                    queue.bytes = 0;
                }
                drop(outcome);
                drop(files);
                drop(directory);
                let mut queue = super::lock(&control.queue);
                queue.done = true;
                drop(queue);
                control.changed.notify_all();
            })
            .map_err(io_error)?;
        *worker.0._thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
        Ok(worker)
    }

    #[cfg(all(test, feature = "log-export"))]
    pub(super) fn hold_release(&self) -> std::sync::mpsc::Sender<()> {
        let (release, gate) = std::sync::mpsc::channel();
        *self.0.release_gate.lock().unwrap() = Some(gate);
        release
    }

    pub(super) fn push(&self, sink: usize, data: Vec<u8>) {
        let mut queue = super::lock(&self.0.queue);
        if queue.closed {
            return;
        }
        if queue.events.len() == 1024 {
            self.0.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        queue.bytes += data.len();
        queue.accepted += 1;
        queue.events.push_back(Event { sink, data });
        drop(queue);
        self.0.changed.notify_one();
    }

    pub(super) fn oversized(&self) {
        self.0.oversized.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn stats(&self) -> LogStats {
        let queue = super::lock(&self.0.queue);
        LogStats {
            queued_events: queue.events.len(),
            queued_bytes: queue.bytes,
            dropped_full: self.0.dropped.load(Ordering::Relaxed),
            oversized: self.0.oversized.load(Ordering::Relaxed),
            write_failures: self.0.failures.load(Ordering::Relaxed),
        }
    }

    pub(super) fn closed(&self) -> bool {
        super::lock(&self.0.queue).closed
    }

    pub(super) fn pending(&self) -> usize {
        usize::from(!super::lock(&self.0.queue).done)
    }

    pub(super) fn request_close(&self) {
        super::lock(&self.0.queue).closed = true;
        self.0.changed.notify_all();
    }

    pub(super) fn wait_closed(&self, until: Instant) -> Result<()> {
        self.wait(until, None)
    }

    pub(super) fn flush(&self, within: Duration) -> Result<()> {
        let until = Instant::now()
            .checked_add(within)
            .unwrap_or_else(Instant::now);
        let serial = {
            let mut queue = super::lock(&self.0.queue);
            queue.flush_serial += 1;
            queue.flush_requested = queue.accepted;
            queue.flush_serial
        };
        self.0.changed.notify_all();
        self.wait(until, Some(serial))
    }

    fn wait(&self, until: Instant, serial: Option<u64>) -> Result<()> {
        let mut queue = super::lock(&self.0.queue);
        loop {
            if queue.done || serial.is_some_and(|s| queue.flush_done >= s) {
                return queue.failure.as_deref().map_or(Ok(()), |e| Err(failure(e)));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if let Some(message) = queue.failure.as_deref() {
                    return Err(failure(message));
                }
                return Err(Error::new(
                    rivium::error::kinds::TIMEOUT,
                    "log worker deadline; resources are still owned",
                ));
            }
            (queue, _) = self
                .0
                .changed
                .wait_timeout(queue, left)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

struct Active {
    name: String,
    file: File,
    size: u64,
    day: String,
}

struct Files {
    active: Vec<Active>,
    archives: BTreeMap<String, u64>,
    size: u64,
    limit: u64,
    file_limit: u64,
    next: u64,
    failed: bool,
}

impl Files {
    fn new(
        directory: &Directory,
        names: Vec<String>,
        settings: &FileSettings,
        reserve: u64,
    ) -> io::Result<Self> {
        for entry in fs::read_dir(&directory.path)? {
            let entry = entry?;
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|n| n.strip_suffix(".tmp"))
                .is_some_and(|n| file_sink(n, &names).is_some())
            {
                reject_symlink(&entry.path())?;
                fs::remove_file(entry.path())?;
            }
        }
        let mut active = Vec::new();
        for name in &names {
            let path = directory.path.join(format!("{name}.log"));
            reject_symlink(&path)?;
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            let metadata = file.metadata()?;
            active.push(Active {
                name: name.clone(),
                size: metadata.len(),
                day: date(metadata.modified().unwrap_or(SystemTime::now())),
                file,
            });
        }
        let mut files = Self {
            active,
            archives: BTreeMap::new(),
            size: 0,
            limit: settings.max_total_size - reserve - settings.max_file_size,
            file_limit: settings.max_file_size,
            next: 0,
            failed: false,
        };
        files.reconcile(&directory.path)?;
        files.next = files
            .archives
            .keys()
            .filter_map(|name| sequence(name))
            .max()
            .unwrap_or(0);
        if files.next == u64::MAX {
            return Err(io::Error::other("archive sequence exhausted"));
        }
        if files.active.iter().any(|a| a.size > files.file_limit)
            || files.archives.values().any(|size| *size > files.file_limit)
        {
            return Err(io::Error::other(
                "existing log file exceeds the configured per-file budget",
            ));
        }
        files.make_room(&directory.path, 0, 0)?;
        Ok(files)
    }

    fn write(&mut self, path: &Path, event: Event) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        let len = event.data.len() as u64;
        if len > self.file_limit {
            return Err(io::Error::other("event exceeds configured log file size"));
        }
        let today = date(SystemTime::now());
        let active = &self.active[event.sink];
        if active.size != 0 && (active.size + len > self.file_limit || active.day != today) {
            self.rotate(path, event.sink)?;
        }
        self.make_room(path, len, 0)?;
        let active = &mut self.active[event.sink];
        active.file.write_all(&event.data)?;
        WRITTEN.fetch_add(len, Ordering::Relaxed);
        active.size += len;
        active.day = today;
        self.size += len;
        Ok(())
    }

    fn rotate(&mut self, path: &Path, sink: usize) -> io::Result<()> {
        let active = &mut self.active[sink];
        active.file.flush()?;
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| io::Error::other("archive sequence exhausted"))?;
        let raw_name = format!("{}.{}.{next:020}.log", active.name, active.day);
        let raw = path.join(&raw_name);
        if raw.try_exists()? {
            return Err(io::Error::other("archive name collision"));
        }
        // Reserve the new archive and the raw/gzip overlap before deleting raw.
        self.make_room(path, 0, 2)?;
        self.next = next;
        let active = &mut self.active[sink];
        fs::rename(path.join(format!("{}.log", active.name)), &raw)?;
        active.file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(path.join(format!("{}.log", active.name)))?;
        let size = std::mem::replace(&mut active.size, 0);
        active.day = date(SystemTime::now());
        self.archives.insert(raw_name.clone(), size);
        // Compression may use at most one raw file of extra disk space. If its
        // output is not smaller, retain the complete raw archive.
        let compressed_name = format!("{raw_name}.gz");
        let temporary = path.join(format!("{compressed_name}.tmp"));
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let mut encoder = flate2::write::GzEncoder::new(
            Limited {
                inner: output,
                remaining: size,
            },
            flate2::Compression::fast(),
        );
        let result = io::copy(&mut File::open(&raw)?, &mut encoder)
            .and_then(|_| encoder.finish().map(|_| ()));
        match result {
            Ok(()) => {
                let compressed_size = fs::metadata(&temporary)?.len();
                if compressed_size < size {
                    fs::rename(&temporary, path.join(&compressed_name))?;
                    fs::remove_file(&raw)?;
                    self.archives.remove(&raw_name);
                    self.archives.insert(compressed_name, compressed_size);
                    self.size -= size - compressed_size;
                } else {
                    fs::remove_file(&temporary)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
                fs::remove_file(&temporary)?;
            }
            Err(error) => {
                fs::remove_file(&temporary)?;
                return Err(error);
            }
        }
        Ok(())
    }

    fn make_room(&mut self, path: &Path, incoming: u64, incoming_files: usize) -> io::Result<()> {
        while self
            .size
            .checked_add(incoming)
            .is_none_or(|n| n > self.limit)
            || self.active.len() + self.archives.len() + incoming_files > MAX_OWNED_FILES
        {
            let Some((name, size)) = self
                .archives
                .iter()
                .min_by_key(|(name, _)| sequence(name).expect("indexed archive has a sequence"))
                .map(|(n, s)| (n.clone(), *s))
            else {
                return Err(io::Error::other("active log files exceed disk budget"));
            };
            fs::remove_file(path.join(&name))?;
            DELETED.fetch_add(1, Ordering::Relaxed);
            self.archives.remove(&name);
            self.size -= size;
        }
        Ok(())
    }

    fn flush(&mut self, path: &Path) -> io::Result<()> {
        for active in &mut self.active {
            active.file.flush()?;
            active.file.sync_data()?;
        }
        self.reconcile(path)?;
        self.make_room(path, 0, 0)
    }

    fn reconcile(&mut self, path: &Path) -> io::Result<()> {
        let names: Vec<_> = self.active.iter().map(|a| a.name.clone()).collect();
        let entries = owned_files(path, &names)?;
        let mut archives = BTreeMap::new();
        let mut total = 0_u64;
        for (name, size) in entries {
            total = total
                .checked_add(size)
                .ok_or_else(|| io::Error::other("log byte accounting overflow"))?;
            if let Some(active) = self
                .active
                .iter_mut()
                .find(|a| name == format!("{}.log", a.name))
            {
                if active.file.metadata()?.len() != size {
                    return Err(io::Error::other("active log file replaced externally"));
                }
                active.size = size;
            } else {
                archives.insert(name, size);
            }
        }
        for active in &self.active {
            if !path.join(format!("{}.log", active.name)).try_exists()? {
                return Err(io::Error::other("active log file removed externally"));
            }
        }
        self.archives = archives;
        self.size = total;
        Ok(())
    }
}

pub(super) struct Limited<T> {
    pub(super) inner: T,
    pub(super) remaining: u64,
}

impl<T: Write> Write for Limited<T> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() as u64 > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "reserved byte limit reached",
            ));
        }
        let n = self.inner.write(data)?;
        self.remaining -= n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(super) fn owned_files(path: &Path, names: &[String]) -> io::Result<Vec<(String, u64)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if file_sink(&name, names).is_some() {
            if !entry.file_type()?.is_file() {
                return Err(io::Error::other("owned log path is not a regular file"));
            }
            if entries.len() == MAX_OWNED_FILES {
                return Err(io::Error::other(
                    "too many owned log files for bounded directory accounting",
                ));
            }
            entries.push((name, entry.metadata()?.len()));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

pub(super) fn file_sink<'a, 'b>(
    file: &'b str,
    names: &'a [String],
) -> Option<(&'a str, Option<&'b str>)> {
    for name in names {
        if file == format!("{name}.log") {
            return Some((name, None));
        }
        if let Some(tail) = file
            .strip_prefix(name.as_str())
            .and_then(|s| s.strip_prefix('.'))
        {
            let raw = tail
                .strip_suffix(".log.gz")
                .or_else(|| tail.strip_suffix(".log"))?;
            if raw.len() == 31
                && raw.is_ascii()
                && valid_date(&raw[..10])
                && raw.as_bytes()[10] == b'.'
                && raw[11..].bytes().all(|b| b.is_ascii_digit())
                && raw[11..].parse::<u64>().is_ok()
            {
                return Some((name, Some(&raw[..10])));
            }
        }
    }
    None
}

pub(super) fn valid_date(value: &str) -> bool {
    if value.len() != 10 || !value.is_ascii() {
        return false;
    }
    let b = value.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        value[..4].parse::<u32>(),
        value[5..7].parse::<u32>(),
        value[8..].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    (1..=max).contains(&day)
}

fn sequence(name: &str) -> Option<u64> {
    name.split('.').nth(2)?.parse().ok()
}

pub(super) fn date(time: SystemTime) -> String {
    // Floor negative timestamps so the last instant before the epoch remains in 1969.
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(value) => i128::from(value.as_secs()),
        Err(error) => {
            -i128::from(error.duration().as_secs())
                - i128::from(error.duration().subsec_nanos() != 0)
        }
    };
    let days = seconds.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i128::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
