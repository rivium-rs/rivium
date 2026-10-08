//! Log export: packs the log files of some days into a zip archive in the log directory, on a
//! thread of its own, for a service to offer for download. One export runs at a time; a finished
//! archive stays until the next export replaces it, it is cancelled, or it expires.

use std::cell::Cell;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rivium_error::{BError, Error, ErrorType, kinds, log_error};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use super::budget::{Kind, is_date, scan};
use super::files::{civil, date, day};
use super::settings::Export;
use crate::Result;

/// How long an export waits for the lines logged before it to reach the files.
const BARRIER: Duration = Duration::from_millis(500);
/// How much an export copies between two looks at a cancellation.
const CHUNK: usize = 64 * 1024;

/// A calendar day, UTC, as the names of rolled log files carry it. It is written `YYYY-MM-DD`,
/// as text and in serde formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Date(i64);

impl Date {
    /// Today, UTC.
    #[must_use]
    pub fn today() -> Date {
        Date::from(SystemTime::now())
    }
}

impl From<SystemTime> for Date {
    /// The day of `time`, UTC.
    fn from(time: SystemTime) -> Date {
        Date(day(time))
    }
}

impl FromStr for Date {
    type Err = BError;

    /// Reads `YYYY-MM-DD`; a day the calendar does not have, such as `2026-02-30`, is an
    /// [`INVALID_INPUT`](kinds::INVALID_INPUT) error.
    fn from_str(text: &str) -> Result<Date> {
        let number = |range: std::ops::Range<usize>| text[range].parse::<i64>().unwrap_or(0);
        if is_date(text) {
            let (y, m, d) = (number(0..4), number(5..7), number(8..10));
            // Days since 1970-01-01, counting years from March (the inverse of `civil`).
            let (y, mp) = (y - i64::from(m <= 2), (m + 9) % 12);
            let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
            let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1;
            let day = era * 146_097 + doe - 719_468;
            // A day the calendar does not have comes out as another one.
            if date(day) == text {
                return Ok(Date(day));
            }
        }
        let why = format!("not a day written like 2026-10-08: {text:?}");
        Error::e_explain(kinds::INVALID_INPUT, why)
    }
}

impl TryFrom<String> for Date {
    type Error = BError;

    fn try_from(text: String) -> Result<Date> {
        text.parse()
    }
}

impl From<Date> for String {
    fn from(day: Date) -> String {
        day.to_string()
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&date(self.0))
    }
}

/// The number of an export, unique in the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExportId(u64);

impl fmt::Display for ExportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What to export: the log files of the days from `from` to `to`, both included, of the log
/// files named in `sinks` (see [`LogExporter::sinks`]), or of every log file when it is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    /// The first day.
    pub from: Date,
    /// The last day.
    pub to: Date,
    /// The names of the log files; empty for all.
    pub sinks: Vec<String>,
}

/// How an export is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Progress {
    /// From 0 to 100, which only a finished export reaches.
    pub percent: u8,
    /// Where the export is.
    pub state: ExportState,
}

/// Where an export is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportState {
    /// Packing.
    Running,
    /// The archive is ready: see [`LogExporter::archive`].
    Done,
    /// Packing failed: [`LogExporter::archive`] says why.
    Failed,
    /// Cancelled; its files are gone.
    Cancelled,
}

/// A finished export's archive.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Archive {
    /// Where it is: `export-<id>.zip` in the log directory.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size: u64,
    /// The name to download it as: `<name>-logs-<from>_<to>.zip`.
    pub file_name: String,
}

/// Packs log files into a zip archive to download, from the service's
/// [`AppContext::log_exporter`](crate::AppContext::log_exporter).
///
/// One export runs at a time, on a thread of its own: [`start`](Self::start) returns at once,
/// and [`progress`](Self::progress) tells when the archive is ready. It contains the chosen log
/// files as they were when the export started, compressed log files as they are, and a
/// `manifest.txt` that lists the files and those the log directory's budget deleted while they
/// were being packed. The archive is at most `log.export.max_size`, a share of the log
/// directory's budget kept for it, and it can be downloaded until the next export replaces it,
/// [`cancel`](Self::cancel) removes it, or `log.export.expire_after` passes. The exporter belongs
/// to the process's logging: a restart in the process does not touch an export in progress, and
/// stopping the host cancels it.
///
/// ```no_run
/// use rivium::log::{Date, ExportRequest, ExportState, LogExporter};
///
/// fn today(exporter: &LogExporter) -> rivium::Result<()> {
///     let today = Date::today();
///     let id = exporter.start(ExportRequest { from: today, to: today, sinks: Vec::new() })?;
///     while exporter.progress(id)?.state == ExportState::Running {
///         std::thread::sleep(std::time::Duration::from_millis(100));
///     }
///     let archive = exporter.archive(id)?;
///     println!("{} bytes in {}", archive.size, archive.path.display());
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct LogExporter(pub(super) Option<Arc<Exports>>);

impl LogExporter {
    /// The names of the log files: the main file, named after the service, then the categories
    /// of `[[log.files]]`.
    #[must_use]
    pub fn sinks(&self) -> Vec<String> {
        let exports = self.0.as_deref();
        exports
            .map(|exports| exports.sinks.clone())
            .unwrap_or_default()
    }

    /// Starts an export and returns its number. Lines logged before the call are in the archive.
    ///
    /// # Errors
    ///
    /// A [`CONFLICT`](kinds::CONFLICT) error while another export runs; a
    /// [`NOT_FOUND`](kinds::NOT_FOUND) error when no log file matches; an
    /// [`INVALID_INPUT`](kinds::INVALID_INPUT) error when `from` is after `to` or a log file name
    /// is unknown; an [`UNAVAILABLE`](kinds::UNAVAILABLE) error when logging is not installed.
    pub fn start(&self, request: ExportRequest) -> Result<ExportId> {
        self.exports()?.start(&request).map(ExportId)
    }

    /// How the export `id` is going.
    ///
    /// # Errors
    ///
    /// A [`NOT_FOUND`](kinds::NOT_FOUND) error when the export was replaced or expired.
    pub fn progress(&self, id: ExportId) -> Result<Progress> {
        self.exports()?.with(id, |task| {
            let percent = match task.state {
                ExportState::Done => 100,
                _ => {
                    let done = u128::from(task.work.done.load(Relaxed)) * 100;
                    (done / u128::from(task.work.total.max(1))).min(99) as u8
                }
            };
            Ok(Progress {
                percent,
                state: task.state,
            })
        })
    }

    /// Cancels the export `id`: stops it and removes what it has written, or removes its
    /// finished archive. Does nothing for an unknown export.
    pub fn cancel(&self, id: ExportId) {
        if let Some(exports) = &self.0 {
            exports.cancel(id.0);
        }
    }

    /// The archive of the export `id`, as often as asked until it goes.
    ///
    /// # Errors
    ///
    /// Why packing failed, such as an [`INVALID_INPUT`](kinds::INVALID_INPUT) error when the
    /// archive would exceed `log.export.max_size`; a [`CONFLICT`](kinds::CONFLICT) error while it
    /// runs; a [`NOT_FOUND`](kinds::NOT_FOUND) error when it was cancelled, replaced or expired.
    pub fn archive(&self, id: ExportId) -> Result<Archive> {
        let exports = self.exports()?;
        exports.with(id, |task| match task.state {
            ExportState::Done => Ok(Archive {
                path: exports.dir.join(format!("export-{id}.zip")),
                size: task.size,
                file_name: task.file_name.clone(),
            }),
            ExportState::Running => {
                Error::e_explain(kinds::CONFLICT, format!("log export {id} is still running"))
            }
            ExportState::Failed => {
                let Failure(kind, why) = task.failure.clone().unwrap_or_default();
                Err(Error::explain(kind, why))
            }
            ExportState::Cancelled => {
                Error::e_explain(kinds::NOT_FOUND, format!("log export {id} was cancelled"))
            }
        })
    }

    fn exports(&self) -> Result<&Arc<Exports>> {
        let why = "logging is not installed, so there are no log files to export";
        (self.0.as_ref()).ok_or_else(|| Error::explain(kinds::UNAVAILABLE, why))
    }
}

impl fmt::Debug for LogExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LogExporter").field(&self.sinks()).finish()
    }
}

/// The exports of the process's log directory.
pub(super) struct Exports {
    /// The service's name: the main log file's.
    name: String,
    dir: PathBuf,
    sinks: Vec<String>,
    max_size: u64,
    expire_after: Duration,
    slot: Mutex<Slot>,
    changed: Condvar,
}

/// The last export.
#[derive(Default)]
struct Slot {
    last: u64,
    task: Option<Task>,
}

struct Task {
    id: u64,
    file_name: String,
    state: ExportState,
    work: Arc<Work>,
    /// Why packing failed.
    failure: Option<Failure>,
    /// The archive's size, once done.
    size: u64,
}

/// What the packing thread shares while it runs.
#[derive(Default)]
struct Work {
    /// The bytes of the chosen files, and how many of them are packed.
    total: u64,
    done: AtomicU64,
    cancel: AtomicBool,
}

/// The chosen files: the active files, opened as the export starts and cut at their length then,
/// and the rolled files and archives, oldest first.
#[derive(Default)]
struct Selection {
    active: Vec<(String, File, u64, SystemTime)>,
    files: Vec<(u64, PathBuf, u64)>,
}

/// Why packing stopped before the end.
enum Stop {
    Cancelled,
    Failed(Failure),
}

/// The kind and message of a failure.
#[derive(Clone)]
struct Failure(ErrorType, String);

impl Default for Failure {
    fn default() -> Self {
        Failure(kinds::INTERNAL, String::new())
    }
}

impl<E: std::error::Error> From<E> for Stop {
    fn from(error: E) -> Self {
        let why = format!("cannot write the log export: {error}");
        Stop::Failed(Failure(kinds::INTERNAL, why))
    }
}

impl Exports {
    pub(super) fn new(name: &str, dir: PathBuf, sinks: Vec<String>, settings: &Export) -> Self {
        Exports {
            name: name.to_string(),
            dir,
            sinks,
            max_size: settings.max_size,
            expire_after: settings.expire_after,
            slot: Mutex::default(),
            changed: Condvar::new(),
        }
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        // Every change completes under the lock.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `read` on the task `id`.
    fn with<T>(&self, id: ExportId, read: impl FnOnce(&Task) -> Result<T>) -> Result<T> {
        match self.slot().task.as_ref().filter(|task| task.id == id.0) {
            Some(task) => read(task),
            None => Error::e_explain(
                kinds::NOT_FOUND,
                format!("no log export {id}: it was replaced or expired"),
            ),
        }
    }

    fn start(self: &Arc<Self>, request: &ExportRequest) -> Result<u64> {
        let ExportRequest { from, to, sinks } = request;
        if from > to {
            let why = format!("the first day {from} is after the last day {to}");
            return Error::e_explain(kinds::INVALID_INPUT, why);
        }
        let unknown: Vec<&String> = sinks.iter().filter(|s| !self.sinks.contains(s)).collect();
        if !unknown.is_empty() {
            let why = format!("no log file named {unknown:?}: they are {:?}", self.sinks);
            return Error::e_explain(kinds::INVALID_INPUT, why);
        }
        let sinks = if sinks.is_empty() { &self.sinks } else { sinks };
        let mut slot = self.slot();
        if let Some(task) = slot.task.as_ref()
            && task.state == ExportState::Running
        {
            let why = format!("log export {} is still running", task.id);
            return Error::e_explain(kinds::CONFLICT, why);
        }
        // An export that cannot wait for the writer still packs what the files hold.
        let _ = super::flush(BARRIER);
        let selection = self.select(*from..=*to, sinks)?;
        // The new archive replaces the last one, which goes first: one export file at a time.
        if let Some(last) = slot.task.take()
            && last.state == ExportState::Done
        {
            let _ = fs::remove_file(self.dir.join(format!("export-{}.zip", last.id)));
        }
        self.changed.notify_all();
        let total = selection.active.iter().map(|active| active.2).sum::<u64>()
            + selection.files.iter().map(|file| file.2).sum::<u64>();
        let count = selection.active.len() + selection.files.len();
        let id = slot.last + 1;
        let work = Arc::new(Work {
            total,
            ..Work::default()
        });
        let (exports, shared) = (Arc::clone(self), Arc::clone(&work));
        std::thread::Builder::new()
            .name(format!("{}-log-export", self.name))
            .spawn(move || exports.run(id, selection, &shared))
            .map_err(|error| Error::because(kinds::INTERNAL, "starting the export", error))?;
        slot.last = id;
        slot.task = Some(Task {
            id,
            file_name: format!("{}-logs-{from}_{to}.zip", self.name),
            state: ExportState::Running,
            work,
            failure: None,
            size: 0,
        });
        tracing::info!(export.id = id, export.from = %from, export.to = %to, export.files = count,
            export.bytes = total, "log export started");
        Ok(id)
    }

    /// The files of `sinks` from the `days`.
    fn select(&self, days: std::ops::RangeInclusive<Date>, sinks: &[String]) -> Result<Selection> {
        let mut selection = Selection::default();
        for entry in scan(&self.dir) {
            if !sinks.contains(&entry.base) {
                continue;
            }
            match entry.kind {
                Kind::Active => {
                    if let Some((name, file, modified)) = open(&entry.path)
                        && let Ok(len) = file.metadata().map(|meta| meta.len())
                        && len > 0
                        && days.contains(&Date::from(modified))
                    {
                        selection.active.push((name, file, len, modified));
                    }
                }
                Kind::Rolled(seq) | Kind::Archive(seq) => {
                    let name = file_name(&entry.path);
                    let day = name.split('.').nth(1).and_then(|day| day.parse().ok());
                    if day.is_some_and(|day| days.contains(&day)) {
                        selection.files.push((seq, entry.path, entry.len));
                    }
                }
                Kind::Partial(_) | Kind::Export => {}
            }
        }
        selection.files.sort();
        if selection.active.is_empty() && selection.files.is_empty() {
            let (from, to) = (days.start(), days.end());
            let why = format!("no log files of {sinks:?} from {from} to {to}");
            return Error::e_explain(kinds::NOT_FOUND, why);
        }
        Ok(selection)
    }

    /// The thread of export `id`: packs, then keeps the archive until it is replaced, cancelled
    /// or expired.
    fn run(&self, id: u64, selection: Selection, work: &Work) {
        let part = self.dir.join(format!("export-{id}.zip.part"));
        let archive = self.dir.join(format!("export-{id}.zip"));
        let packed = (self.pack(&part, selection, work))
            .and_then(|packed| Ok(fs::rename(&part, &archive).map(|()| packed)?));
        if packed.is_err() {
            let _ = fs::remove_file(&part);
        }
        let mut slot = self.slot();
        let Some(task) = slot.task.as_mut().filter(|task| task.id == id) else {
            return;
        };
        match packed {
            Ok((size, skipped)) => {
                (task.state, task.size) = (ExportState::Done, size);
                let path = archive.display();
                tracing::info!(export.id = id, export.size = size, export.skipped = skipped,
                    export.path = %path, "log export finished");
            }
            Err(Stop::Cancelled) => {
                task.state = ExportState::Cancelled;
                tracing::info!(export.id = id, "log export cancelled");
            }
            Err(Stop::Failed(failure)) => {
                let error = Error::explain(failure.0, failure.1.clone());
                log_error!(
                    tracing::Level::WARN,
                    &error,
                    export.id = id,
                    "log export failed"
                );
                (task.state, task.failure) = (ExportState::Failed, Some(failure));
            }
        }
        self.changed.notify_all();
        let ours = |slot: &mut Slot| {
            (slot.task.as_ref())
                .is_some_and(|task| task.id == id && task.state == ExportState::Done)
        };
        let waited = self
            .changed
            .wait_timeout_while(slot, self.expire_after, ours);
        let (mut slot, waited) = waited.unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() && ours(&mut slot) {
            let _ = fs::remove_file(&archive);
            slot.task = None;
            tracing::info!(export.id = id, "log export expired");
        }
    }

    /// Writes the archive at `part`; returns its size and the number of files skipped.
    fn pack(&self, part: &Path, selection: Selection, work: &Work) -> Result<(u64, usize), Stop> {
        let over = Cell::new(false);
        let file = File::create(part)?;
        let mut zip = ZipWriter::new_stream(Limited {
            file: BufWriter::new(file),
            written: 0,
            max: self.max_size,
            over: &over,
        });
        let mut packed = Vec::new();
        let mut skipped = Vec::new();
        let packing = (|| {
            for (name, file, len, modified) in selection.active {
                add(&mut zip, &name, modified, file.take(len), work)?;
                packed.push(name);
            }
            for (_, path, len) in selection.files {
                // A rolled file that was compressed meanwhile is packed as its archive.
                let gz = path.with_extension("log.gz");
                match open(&path).or_else(|| open(&gz)) {
                    Some((name, file, modified)) => {
                        add(&mut zip, &name, modified, file, work)?;
                        packed.push(name);
                    }
                    None => {
                        work.done.fetch_add(len, Relaxed);
                        skipped.push(file_name(&path));
                    }
                }
            }
            let manifest = manifest(&self.name, &packed, &skipped);
            zip.start_file("manifest.txt", options("manifest.txt", SystemTime::now()))?;
            zip.write_all(manifest.as_bytes())?;
            let limited = zip.finish()?.into_inner();
            let file = limited
                .file
                .into_inner()
                .map_err(io::IntoInnerError::into_error);
            file?.sync_all()?;
            Ok::<_, Stop>(limited.written)
        })();
        match packing {
            Ok(size) => Ok((size, skipped.len())),
            Err(Stop::Failed(_)) if over.get() => {
                let max = crate::config::de::format_bytes(self.max_size);
                let why = format!(
                    "the archive would exceed log.export.max_size ({max}): choose fewer days or log files"
                );
                Err(Stop::Failed(Failure(kinds::INVALID_INPUT, why)))
            }
            Err(stop) => Err(stop),
        }
    }

    fn cancel(&self, id: u64) {
        let mut slot = self.slot();
        let Some(task) = slot.task.as_mut().filter(|task| task.id == id) else {
            return;
        };
        match task.state {
            ExportState::Running => task.work.cancel.store(true, Relaxed),
            ExportState::Done => {
                let _ = fs::remove_file(self.dir.join(format!("export-{id}.zip")));
                task.state = ExportState::Cancelled;
                self.changed.notify_all();
                tracing::info!(export.id = id, "log export cancelled");
            }
            ExportState::Failed | ExportState::Cancelled => {}
        }
    }

    /// Cancels the export in progress, if any, and waits up to `within` for it to end.
    pub(super) fn cancel_running(&self, within: Duration) {
        let slot = self.slot();
        // Only packing looks at the flag.
        if let Some(task) = &slot.task {
            task.work.cancel.store(true, Relaxed);
        }
        let running = |slot: &mut Slot| {
            (slot.task.as_ref()).is_some_and(|task| task.state == ExportState::Running)
        };
        drop(self.changed.wait_timeout_while(slot, within, running));
    }
}

/// The name of a file, and the file opened with the time it was last written.
fn open(path: &Path) -> Option<(String, File, SystemTime)> {
    let file = File::open(path).ok()?;
    let modified = file.metadata().and_then(|meta| meta.modified()).ok()?;
    Some((file_name(path), file, modified))
}

fn file_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default();
    name.to_string_lossy().into_owned()
}

/// Adds one file, `name`, read from `file`; stops early when the export is cancelled.
fn add<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    modified: SystemTime,
    mut file: impl Read,
    work: &Work,
) -> Result<(), Stop> {
    zip.start_file(name, options(name, modified))?;
    let mut buffer = vec![0; CHUNK];
    loop {
        if work.cancel.load(Relaxed) {
            return Err(Stop::Cancelled);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        zip.write_all(&buffer[..read])?;
        work.done.fetch_add(read as u64, Relaxed);
    }
}

/// Compressed files are stored as they are; other files are deflated. Entries keep the time
/// their file was last written, UTC.
fn options(name: &str, modified: SystemTime) -> SimpleFileOptions {
    let method = match name.ends_with(".gz") {
        true => CompressionMethod::Stored,
        false => CompressionMethod::Deflated,
    };
    let since = modified.duration_since(UNIX_EPOCH);
    let seconds = since.map_or(0, |since| since.as_secs());
    let (y, m, d) = civil((seconds / 86_400) as i64);
    let (h, mi, s) = (seconds / 3_600 % 24, seconds / 60 % 60, seconds % 60);
    let time = DateTime::from_date_and_time(y as u16, m as u8, d as u8, h as u8, mi as u8, s as u8);
    let options = SimpleFileOptions::default().compression_method(method);
    options.last_modified_time(time.unwrap_or_default())
}

/// What the archive contains, and the chosen files that the budget deleted before they were
/// packed.
fn manifest(name: &str, packed: &[String], skipped: &[String]) -> String {
    let mut text = format!("Log export of {name}, made {}.\n\nPacked:\n", Date::today());
    for file in packed {
        text += &format!("  {file}\n");
    }
    if !skipped.is_empty() {
        text += "\nDeleted by the log directory's budget before they were packed:\n";
        for file in skipped {
            text += &format!("  {file}\n");
        }
    }
    text
}

/// The archive's file, which refuses a write that would take it over `max`.
struct Limited<'a> {
    file: BufWriter<File>,
    written: u64,
    max: u64,
    over: &'a Cell<bool>,
}

impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written + bytes.len() as u64 > self.max {
            self.over.set(true);
            return Err(io::Error::other("the archive is too large"));
        }
        let written = self.file.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests;
