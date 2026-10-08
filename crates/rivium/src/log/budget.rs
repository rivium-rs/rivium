//! The log directory: file names, the disk budget, and the thread that compresses archives.
//!
//! Names: the active file `<base>.log`, rolled files `<base>.<YYYY-MM-DD>.<N>.log`, archives
//! `<base>.<YYYY-MM-DD>.<N>.log.gz`, archives being written `….log.gz.tmp`. `N` is a sequence
//! number unique in the directory; files are deleted in its order, never by date or time, so a
//! device clock that jumps cannot make the newest logs go first.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};

use flate2::Compression;
use flate2::write::GzEncoder;

use super::files::Counters;

/// What a file in the log directory is, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// `<base>.log`.
    Active,
    /// `<base>.<date>.<N>.log`, waiting to be compressed.
    Rolled(u64),
    /// `<base>.<date>.<N>.log.gz`.
    Archive(u64),
    /// `<base>.<date>.<N>.log.gz.tmp`, being compressed.
    Partial(u64),
    /// `export-<id>.zip` or `export-<id>.zip.part`.
    Export,
}

/// The base name and kind of a file named as the log directory names its files.
pub(super) fn parse(name: &str) -> Option<(&str, Kind)> {
    if name.starts_with("export-") && (name.ends_with(".zip") || name.ends_with(".zip.part")) {
        return Some(("", Kind::Export));
    }
    let (stem, suffix) = [".log.gz.tmp", ".log.gz", ".log"]
        .into_iter()
        .find_map(|suffix| Some((name.strip_suffix(suffix)?, suffix)))?;
    let parts: Vec<&str> = stem.split('.').collect();
    let base_ok = |base: &str| !base.is_empty() && !base.contains(['/', '\\']);
    match (parts.as_slice(), suffix) {
        ([base], ".log") if base_ok(base) => Some((base, Kind::Active)),
        ([base, date, seq], _) if base_ok(base) && is_date(date) => {
            let seq = seq
                .parse()
                .ok()
                .filter(|_| seq.bytes().all(|b| b.is_ascii_digit()))?;
            let kind = match suffix {
                ".log" => Kind::Rolled(seq),
                ".log.gz" => Kind::Archive(seq),
                _ => Kind::Partial(seq),
            };
            Some((base, kind))
        }
        _ => None,
    }
}

pub(super) fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && (bytes.iter().enumerate()).all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// A file of the log directory.
#[derive(Debug)]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) base: String,
    pub(super) kind: Kind,
    pub(super) len: u64,
}

/// The files of the log directory that are named as it names them.
pub(super) fn scan(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    (read.filter_map(Result::ok))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (base, kind) = parse(&name)?;
            let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            Some(Entry {
                path: entry.path(),
                base: base.to_string(),
                kind,
                len: meta.len(),
            })
        })
        .collect()
}

/// The disk budget of one log directory, shared by the writer and the compressing thread.
pub(super) struct Budget {
    pub(super) dir: PathBuf,
    /// The base names of the active files, which are never deleted.
    pub(super) sinks: Vec<String>,
    /// The share of the log files: active, rolled and compressed ones.
    pub(super) target: u64,
    pub(super) counters: Arc<Counters>,
    /// The rolled file being compressed, which is not deleted until it is done. Held for the
    /// whole of an enforcement, so the compressor never starts on a file being deleted.
    pub(super) compressing: Mutex<Option<PathBuf>>,
}

impl Budget {
    /// Deletes rolled files and archives, lowest sequence number first, until the log files fit
    /// in their share. Active files and the file being compressed are kept; files that are not
    /// named as the log directory names its files are neither counted nor deleted.
    pub(super) fn enforce(&self) {
        let compressing = self
            .compressing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let entries = scan(&self.dir);
        let active = |e: &Entry| e.kind == Kind::Active && self.sinks.contains(&e.base);
        let seq = |e: &Entry| match e.kind {
            Kind::Rolled(seq) | Kind::Archive(seq) => Some(seq),
            _ => None,
        };
        let counted = entries.iter().filter(|e| active(e) || seq(e).is_some());
        let mut total: u64 = counted.map(|e| e.len).sum();
        let mut deletable: Vec<&Entry> = (entries.iter())
            .filter(|e| seq(e).is_some() && compressing.as_deref() != Some(&*e.path))
            .collect();
        deletable.sort_by_key(|e| seq(e));
        for entry in deletable {
            if total <= self.target {
                break;
            }
            if fs::remove_file(&entry.path).is_ok() {
                total = total.saturating_sub(entry.len);
                self.counters.deleted.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Compresses each rolled file it receives into an archive, then enforces the budget; skips
    /// files the budget already deleted. Ends when the writer is gone.
    pub(super) fn compress(&self, rolled: &Receiver<PathBuf>) {
        while let Ok(path) = rolled.recv() {
            {
                let mut compressing = self
                    .compressing
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if !path.exists() {
                    continue;
                }
                *compressing = Some(path.clone());
            }
            let mut tmp = path.clone().into_os_string();
            tmp.push(".gz.tmp");
            let tmp = PathBuf::from(tmp);
            let done = gzip(&path, &tmp).and_then(|()| fs::rename(&tmp, tmp.with_extension("")));
            match done {
                Ok(()) => drop(fs::remove_file(&path)),
                Err(_) => {
                    let _ = fs::remove_file(&tmp);
                    self.counters.failures.fetch_add(1, Ordering::Relaxed);
                }
            }
            *self
                .compressing
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            self.enforce();
        }
    }
}

fn gzip(from: &Path, to: &Path) -> io::Result<()> {
    let mut input = BufReader::new(File::open(from)?);
    let mut output = GzEncoder::new(BufWriter::new(File::create(to)?), Compression::default());
    io::copy(&mut input, &mut output)?;
    output
        .finish()?
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()
}
