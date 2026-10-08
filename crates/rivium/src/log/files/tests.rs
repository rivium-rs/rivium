//! The log files: names, dates, rolling, the budget, the flush barrier, and a storm that must
//! keep the directory within its bound.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::budget::{Budget, Kind, parse, scan};
use super::{Clock, Counters, Files, FilesConfig, date, day};

const KIB: u64 = 1 << 10;
const DAY: u64 = 86_400;
/// 2026-10-08T12:00:00Z.
const NOON: u64 = 1_791_460_800;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rivium-log-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn config(dir: &Path, sinks: &[&str], max_file_size: u64, max_total_size: u64) -> FilesConfig {
    let sinks = sinks.iter().map(ToString::to_string).collect();
    FilesConfig {
        dir: dir.to_path_buf(),
        sinks,
        max_file_size,
        max_total_size,
        export_reserve: 0,
    }
}

/// A clock that tests move by setting the seconds since the epoch.
fn manual(seconds: u64) -> (Clock, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(seconds));
    let read = Arc::clone(&now);
    (
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(read.load(Ordering::Relaxed))),
        now,
    )
}

fn real() -> Clock {
    Arc::new(SystemTime::now)
}

/// The names in the directory.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = (fs::read_dir(dir).unwrap())
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// The sequence numbers of the rolled files and archives in the directory.
fn sequences(dir: &Path) -> BTreeSet<u64> {
    (scan(dir).into_iter())
        .filter_map(|entry| match entry.kind {
            Kind::Rolled(seq) | Kind::Archive(seq) | Kind::Partial(seq) => Some(seq),
            _ => None,
        })
        .collect()
}

/// The bytes of the files in the directory, each file counted once even if a rename makes it
/// appear under two names while the directory is read.
fn usage(dir: &Path) -> u64 {
    let mut seen = BTreeSet::new();
    let entries = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok);
    let files = entries.filter_map(|entry| entry.metadata().ok());
    files
        .filter(|meta| meta.is_file() && seen.insert(identity(meta)))
        .map(|meta| meta.len())
        .sum()
}

#[cfg(unix)]
fn identity(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(meta: &fs::Metadata) -> (u64, u64) {
    // No stable file index on Windows: a duplicate name could count twice, see `bounded`.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    (meta.len(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A line of `len` bytes: the text, then hexadecimal noise that compresses to about half, so
/// archives take room and the budget has work to do.
fn line(text: &str, len: usize) -> Vec<u8> {
    let mut line = format!("{text} ").into_bytes();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ (len as u64) ^ text.len() as u64;
    for byte in text.bytes() {
        state = (state ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
    }
    while line.len() < len - 1 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        line.push(b"0123456789abcdef"[(state & 15) as usize]);
    }
    line.push(b'\n');
    line
}

#[test]
fn names_follow_the_rules_of_the_directory() {
    let cases = [
        ("app.log", Some(("app", Kind::Active))),
        ("app.2026-10-08.7.log", Some(("app", Kind::Rolled(7)))),
        ("app.2026-10-08.7.log.gz", Some(("app", Kind::Archive(7)))),
        (
            "app.2026-10-08.7.log.gz.tmp",
            Some(("app", Kind::Partial(7))),
        ),
        ("export-1.zip", Some(("", Kind::Export))),
        ("export-1.zip.part", Some(("", Kind::Export))),
        ("app.2026-10-8.7.log", None),
        ("app.2026-10-08.+7.log", None),
        ("app.2026-10-08.log", None),
        ("app.x.log", None),
        (".log", None),
        ("notes.txt", None),
        ("app.log.gz", None),
    ];
    for (name, expected) in cases {
        assert_eq!(parse(name), expected, "{name}");
    }
}

#[test]
fn dates_are_utc_calendar_days() {
    let at = |seconds: i64| {
        let time = match u64::try_from(seconds) {
            Ok(after) => UNIX_EPOCH + Duration::from_secs(after),
            Err(_) => UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs()),
        };
        date(day(time))
    };
    assert_eq!(at(0), "1970-01-01");
    assert_eq!(at(-1), "1969-12-31");
    assert_eq!(at(951_782_400), "2000-02-29");
    assert_eq!(at(1_704_067_199), "2023-12-31");
    assert_eq!(at(1_704_067_200), "2024-01-01");
    assert_eq!(at(NOON as i64), "2026-10-08");
    assert_eq!(at(4_107_542_400), "2100-03-01");
}

#[test]
fn the_budget_deletes_by_sequence_and_keeps_active_and_compressing_files() {
    let dir = dir("budget");
    fs::create_dir_all(&dir).unwrap();
    let files = [
        ("app.log", 300),
        ("app.2026-01-01.1.log.gz", 200),
        ("app.2026-01-02.2.log", 200),
        ("old.2026-01-01.3.log.gz", 100),
        ("app.2025-12-31.4.log", 100),
        ("old.log", 500),
        ("notes.txt", 1_000),
    ];
    for (name, len) in files {
        fs::write(dir.join(name), vec![b'x'; len]).unwrap();
    }
    let budget = Budget {
        dir: dir.clone(),
        sinks: vec!["app".into()],
        target: 600,
        counters: Arc::new(Counters::default()),
        compressing: std::sync::Mutex::new(Some(dir.join("app.2026-01-02.2.log"))),
    };
    // Counted: the active file and every rolled file or archive, of any base: 900 bytes.
    // Sequence 2 is being compressed, so 1 and 3 go; 4 is newer, whatever its date.
    budget.enforce();
    let expected = [
        "app.2025-12-31.4.log",
        "app.2026-01-02.2.log",
        "app.log",
        "notes.txt",
        "old.log",
    ];
    assert_eq!(names(&dir), expected);
    assert_eq!(budget.counters.deleted.load(Ordering::Relaxed), 2);
    // At the target, nothing more goes; below it, the archive goes once it is no longer being
    // compressed.
    let budget = Budget {
        target: 400,
        compressing: std::sync::Mutex::new(None),
        ..budget
    };
    budget.enforce();
    assert_eq!(
        names(&dir),
        ["app.2025-12-31.4.log", "app.log", "notes.txt", "old.log"]
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_roll_by_size_and_by_day_and_numbers_keep_counting() {
    let dir = dir("roll");
    let (clock, now) = manual(NOON);
    let files = Files::start(config(&dir, &["app", "proto"], 4 * KIB, 64 * KIB), clock).unwrap();
    let (app, proto) = (files.sender(0), files.sender(1));
    for n in 0..5 {
        assert!(app.send(line(&format!("app {n}"), 1_000)));
    }
    assert!(proto.send(line("proto", 100)));
    // The fifth line does not fit in 4 KiB: the first four were rolled. The writer reads the
    // clock as it writes, so each move waits for the lines before it.
    assert!(files.flush(Duration::from_secs(10)));
    now.store(NOON + DAY, Ordering::Relaxed);
    assert!(app.send(line("next day", 100)));
    assert!(files.flush(Duration::from_secs(10)));
    now.store(NOON - 3 * DAY, Ordering::Relaxed);
    assert!(app.send(line("clock moved back", 100)));
    assert!(files.flush(Duration::from_secs(10)));
    drop((app, proto));
    files.stop();
    let expected = [
        "app.2026-10-08.1.log.gz",
        "app.2026-10-08.2.log.gz",
        "app.2026-10-09.3.log.gz",
        "app.log",
        "proto.log",
    ];
    assert_eq!(names(&dir), expected);
    assert!(
        fs::read_to_string(dir.join("app.log"))
            .unwrap()
            .starts_with("clock moved back")
    );

    // A new run continues the sequence and rolls active files last written on an earlier day.
    for name in ["app.log", "proto.log"] {
        let file = fs::File::options()
            .append(true)
            .open(dir.join(name))
            .unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(NOON))
            .unwrap();
    }
    let (clock, _) = manual(NOON + 10 * DAY);
    let files = Files::start(config(&dir, &["app", "proto"], 4 * KIB, 64 * KIB), clock).unwrap();
    files.stop();
    let rolled: Vec<String> = names(&dir)
        .into_iter()
        .filter(|name| name.contains(".4.") || name.contains(".5."))
        .collect();
    assert_eq!(
        rolled,
        ["app.2026-10-08.4.log.gz", "proto.2026-10-08.5.log.gz"]
    );
    assert_eq!(sequences(&dir), BTreeSet::from([1, 2, 3, 4, 5]));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn startup_compresses_leftovers_and_removes_partial_files() {
    let dir = dir("startup");
    fs::create_dir_all(&dir).unwrap();
    let leftovers = [
        ("app.2026-10-01.3.log", "rolled before a crash\n"),
        ("app.2026-10-01.4.log.gz.tmp", "half compressed"),
        ("export-9.zip.part", "half exported"),
        ("export-8.zip", "an old export"),
        ("app.log", "today\n"),
        ("keep.txt", "not ours"),
    ];
    for (name, text) in leftovers {
        fs::write(dir.join(name), text).unwrap();
    }
    let files = Files::start(config(&dir, &["app"], 4 * KIB, 64 * KIB), real()).unwrap();
    let sender = files.sender(0);
    assert!(sender.send(b"appended\n".to_vec()));
    assert!(files.flush(Duration::from_secs(10)));
    drop(sender);
    files.stop();
    assert_eq!(
        names(&dir),
        ["app.2026-10-01.3.log.gz", "app.log", "keep.txt"]
    );
    assert_eq!(
        fs::read_to_string(dir.join("app.log")).unwrap(),
        "today\nappended\n"
    );
    let mut text = String::new();
    let archive = fs::File::open(dir.join("app.2026-10-01.3.log.gz")).unwrap();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(archive), &mut text).unwrap();
    assert_eq!(text, "rolled before a crash\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_deleted_file_or_directory_comes_back_within_a_second() {
    let dir = dir("deleted");
    let files = Files::start(config(&dir, &["app"], 64 * KIB, 1024 * KIB), real()).unwrap();
    let sender = files.sender(0);
    assert!(sender.send(b"before\n".to_vec()));
    assert!(files.flush(Duration::from_secs(10)));
    if fs::remove_dir_all(&dir).is_err() {
        // Windows may refuse to remove the directory of an open file; then there is nothing to
        // bring back.
        drop(sender);
        return files.stop();
    }
    std::thread::sleep(Duration::from_millis(1_100));
    assert!(sender.send(b"after\n".to_vec()));
    assert!(files.flush(Duration::from_secs(10)));
    assert_eq!(fs::read_to_string(dir.join("app.log")).unwrap(), "after\n");
    drop(sender);
    files.stop();
    fs::remove_dir_all(&dir).unwrap();
}

/// Makes renames in `dir` fail while the guard lives: on Unix by making the directory read
/// only, on Windows by holding the file open without sharing delete access, as another
/// process might. `None` when the platform cannot be made to fail (running as root).
#[cfg(unix)]
fn block_renames(dir: &Path, _file: &Path) -> Option<Box<dyn std::any::Any>> {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let guard = Restore(dir.to_path_buf());
    let probe = dir.join("probe");
    if fs::write(&probe, b"").is_ok() {
        let _ = fs::remove_file(probe);
        return None;
    }
    Some(Box::new(guard))
}

#[cfg(windows)]
fn block_renames(_dir: &Path, file: &Path) -> Option<Box<dyn std::any::Any>> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE. Never skipped on Windows.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(file)
        .expect("open the active file without sharing delete access");
    Some(Box::new(held))
}

#[test]
fn a_failed_rename_keeps_writing_and_is_retried() {
    let dir = dir("rename");
    let files = Files::start(config(&dir, &["app"], 2 * KIB, 64 * KIB), real()).unwrap();
    let sender = files.sender(0);
    assert!(sender.send(line("first", 1_500)));
    assert!(files.flush(Duration::from_secs(10)));
    let Some(guard) = block_renames(&dir, &dir.join("app.log")) else {
        eprintln!("skipped: renames cannot be made to fail here (running as root?)");
        return;
    };
    assert!(sender.send(line("second", 1_500)));
    assert!(files.flush(Duration::from_secs(10)));
    // Nothing was rolled; the line went to the active file, over its size limit.
    assert_eq!(names(&dir), ["app.log"]);
    assert_eq!(fs::metadata(dir.join("app.log")).unwrap().len(), 3_000);
    assert!(files.counters.failures.load(Ordering::Relaxed) >= 1);
    drop(guard);
    std::thread::sleep(Duration::from_millis(1_100));
    assert!(sender.send(line("third", 1_500)));
    assert!(files.flush(Duration::from_secs(10)));
    drop(sender);
    files.stop();
    assert_eq!(sequences(&dir).len(), 1, "{:?}", names(&dir));
    assert_eq!(
        fs::read_to_string(dir.join("app.log")).unwrap(),
        String::from_utf8(line("third", 1_500)).unwrap()
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_event_larger_than_a_file_is_cut_at_a_character_boundary() {
    let dir = dir("large");
    let files = Files::start(config(&dir, &["app"], 1_024, 64 * KIB), real()).unwrap();
    let sender = files.sender(0);
    let mut big = "é".repeat(1_000).into_bytes();
    big.push(b'\n');
    assert!(sender.send(big));
    drop(sender);
    files.stop();
    let text = fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(
        text.len() <= 1_024 && text.ends_with("é\n"),
        "{}",
        text.len()
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// Samples the directory every 20 ms while a storm runs, and checks each sample against the
/// bound. A sample over the bound is taken again at once before it counts, since a rename
/// can show one file under two names while the directory is read on platforms where files
/// cannot be told apart.
fn bounded(dir: &Path, bound: u64, running: &AtomicBool) -> (u64, u64, Vec<u64>) {
    let (mut peak, mut samples, mut highest) = (0, 0, Vec::new());
    while running.load(Ordering::Relaxed) {
        let used = (0..3)
            .map(|_| usage(dir))
            .find(|used| *used <= bound)
            .unwrap_or_else(|| usage(dir));
        assert!(
            used <= bound,
            "the log directory holds {used} bytes, over the bound of {bound}"
        );
        peak = peak.max(used);
        samples += 1;
        highest.push(sequences(dir).last().copied().unwrap_or(0));
        std::thread::sleep(Duration::from_millis(20));
    }
    (peak, samples, highest)
}

#[test]
fn a_storm_keeps_the_directory_within_its_bound() {
    let dir = dir("storm");
    let (max_file, max_total, sinks) = (64 * KIB, 512 * KIB, 2);
    let bound = max_total + sinks * max_file;
    let files = Files::start(config(&dir, &["app", "proto"], max_file, max_total), real()).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let sampler = {
        let (dir, running) = (dir.clone(), Arc::clone(&running));
        std::thread::spawn(move || bounded(&dir, bound, &running))
    };
    // Eight threads write as fast as they can for two seconds, whatever the platform's speed.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let writers: Vec<_> = (0..8)
        .map(|thread| {
            let sender = files.sender(thread % 2);
            std::thread::spawn(move || {
                let mut n = 0;
                while std::time::Instant::now() < deadline {
                    assert!(sender.send(line(&format!("thread {thread} line {n}"), 1_000)));
                    n += 1;
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let last = [files.sender(0), files.sender(1)];
    for (sink, sender) in last.iter().enumerate() {
        assert!(sender.send(format!("last line of sink {sink}\n").into_bytes()));
    }
    assert!(
        files.flush(Duration::from_secs(10)),
        "the flush barrier timed out"
    );
    for (sink, name) in ["app.log", "proto.log"].iter().enumerate() {
        let text = fs::read_to_string(dir.join(name)).unwrap();
        assert!(
            text.ends_with(&format!("last line of sink {sink}\n")),
            "{name} misses its last line"
        );
    }
    drop(last);
    let counters = Arc::clone(&files.counters);
    files.stop();
    running.store(false, Ordering::Relaxed);
    let (peak, samples, highest) = sampler.join().unwrap();
    assert!(samples > 0 && usage(&dir) <= bound);
    // Sequence numbers only grow, and the budget deleted the oldest.
    assert!(highest.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(counters.deleted.load(Ordering::Relaxed) > 0);
    assert_eq!(counters.failures.load(Ordering::Relaxed), 0);
    let written = counters.bytes.load(Ordering::Relaxed);
    assert!(
        written >= 4 * bound,
        "only {written} bytes written: the budget was not exercised"
    );
    // What is left are the newest files: the budget deleted from the lowest number up, except
    // that the one file being compressed at the time may have been kept.
    let left = sequences(&dir);
    let rolled = *left.last().unwrap();
    let newest: Vec<u64> = left.iter().copied().skip(1).collect();
    let run = newest.first().map_or(0, |first| rolled - first + 1);
    assert_eq!(newest.len() as u64, run, "{:?}", names(&dir));
    eprintln!(
        "storm: {written} bytes, peak {peak} of {bound} over {samples} samples, {rolled} rolled"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn deletion_follows_sequence_numbers_when_the_clock_moves_back() {
    let dir = dir("clock");
    let (clock, now) = manual(NOON);
    // Room for about three archives besides the active file.
    let files = Files::start(config(&dir, &["app"], 2 * KIB, 8 * KIB), clock).unwrap();
    let sender = files.sender(0);
    let mut dates = BTreeMap::new();
    for n in 0..12 {
        if n == 6 {
            now.store(NOON - 30 * DAY, Ordering::Relaxed);
        }
        assert!(sender.send(line(&format!("line {n}"), 1_500)));
        assert!(files.flush(Duration::from_secs(10)));
        for entry in scan(&dir) {
            if let Kind::Rolled(seq) | Kind::Archive(seq) = entry.kind {
                let name = entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                dates
                    .entry(seq)
                    .or_insert_with(|| name.split('.').nth(1).unwrap().to_string());
            }
        }
    }
    drop(sender);
    files.stop();
    let left = sequences(&dir);
    let deleted: BTreeSet<u64> = dates
        .keys()
        .copied()
        .filter(|seq| !left.contains(seq))
        .collect();
    // Whatever their dates, every deleted file is older than every file left: files dated
    // 2026-10-08 went before files dated a month earlier.
    assert!(
        !deleted.is_empty() && deleted.last() < left.first(),
        "{deleted:?} {left:?}"
    );
    let latest_left = left.iter().map(|seq| dates[seq].as_str()).min().unwrap();
    assert!(
        deleted.iter().any(|seq| dates[seq].as_str() > latest_left),
        "{dates:?} {left:?}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn startup_keeps_the_export_share_free() {
    let dir = dir("export-share");
    fs::create_dir_all(&dir).unwrap();
    for seq in 1..=5 {
        let name = format!("app.2026-10-01.{seq}.log.gz");
        fs::write(dir.join(name), vec![0; 4_096]).unwrap();
    }
    // 24 KiB less one file size for compression and 8 KiB for log export: 12 KiB for logs.
    let config = FilesConfig {
        export_reserve: 8 * KIB,
        ..config(&dir, &["app"], 4 * KIB, 24 * KIB)
    };
    let files = Files::start(config, real()).unwrap();
    files.stop();
    assert_eq!(sequences(&dir), BTreeSet::from([3, 4, 5]));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn startup_keeps_one_file_size_free_for_compression() {
    let dir = dir("share");
    fs::create_dir_all(&dir).unwrap();
    // Five archives of 4 KiB: with a 16 KiB budget and 4 KiB files, the log files keep 12 KiB,
    // since one file size stays free for the archive being compressed.
    for seq in 1..=5 {
        fs::write(
            dir.join(format!("app.2026-10-01.{seq}.log.gz")),
            vec![0; 4_096],
        )
        .unwrap();
    }
    let files = Files::start(config(&dir, &["app"], 4 * KIB, 16 * KIB), real()).unwrap();
    files.stop();
    assert_eq!(sequences(&dir), BTreeSet::from([3, 4, 5]));
    fs::remove_dir_all(&dir).unwrap();
}
