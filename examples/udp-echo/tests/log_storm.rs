//! A storm of log lines from the real program (S-8, V-14): datagrams of random text from four
//! threads, each logged at `debug`, while the log directory is measured every 20ms. It stays
//! within the budget plus one file per sink while the budget deletes rolled files and a log
//! export packs them, and the program's last line is in the file once it exits. Not built for
//! Android, where services run embedded.
#![cfg(not(target_os = "android"))]

use std::io::Read;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rivium_test::process::spawn;

const BIN: &str = env!("CARGO_BIN_EXE_udp-echo");
const MIB: u64 = 1 << 20;

/// The size of the files in `dir`, the lowest and highest sequence numbers of the rolled
/// files, and whether an export file is there.
fn measure(dir: &Path) -> (u64, u64, u64, bool) {
    let (mut size, mut lowest, mut highest, mut export) = (0, u64::MAX, 0, false);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        // A file deleted while it is measured counts as gone.
        size += entry.metadata().map_or(0, |meta| meta.len());
        let name = entry.file_name().to_string_lossy().into_owned();
        export |= name.starts_with("export-");
        // `udp-echo.<date>.<N>.log` and its archive `….log.gz`.
        let parts: Vec<&str> = name.split('.').collect();
        if let [_, _, n, "log", ..] = parts[..]
            && let Ok(n) = n.parse()
        {
            (lowest, highest) = (lowest.min(n), highest.max(n));
        }
    }
    (size, lowest, highest, export)
}

/// What the measurements found: the largest size, the latest lowest and highest sequence
/// numbers, and whether an export file was seen.
#[derive(Default)]
struct Found {
    largest: u64,
    lowest: u64,
    highest: u64,
    export: bool,
}

/// Measures `dir` every 20ms until `stop`, checking each size against `bound`.
fn sample(dir: &Path, bound: u64, found: &Mutex<Found>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let (size, lowest, highest, export) = measure(dir);
        assert!(
            size <= bound,
            "the log directory holds {size} bytes, more than {bound}"
        );
        let mut found = found.lock().unwrap();
        found.largest = found.largest.max(size);
        (found.lowest, found.highest) = (lowest, highest);
        found.export |= export;
        drop(found);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Sends datagrams of random text, which the program logs and which compresses poorly, so
/// that the rolled files fill the budget.
fn flood(addr: SocketAddr, stop: &AtomicBool, seed: u64) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (mut state, mut datagram) = (seed, [0; 64]);
    while !stop.load(Ordering::Relaxed) {
        for byte in &mut datagram {
            // xorshift
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = b"0123456789abcdefghijklmnopqrstuvwxyz"[(state % 36) as usize];
        }
        // Datagrams that do not fit in the buffers are lost; that is fine.
        let _ = socket.send_to(&datagram, addr);
    }
}

/// Sends `datagram` every 100ms until `done`: the storm may drop some.
fn keep_sending(addr: SocketAddr, datagram: &'static [u8], done: &Arc<AtomicBool>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let done = Arc::clone(done);
    std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            let _ = socket.send_to(datagram, addr);
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

#[test]
fn a_log_storm_stays_within_the_budget_while_an_export_packs_it() {
    let root = std::env::temp_dir().join(format!("udp-echo-storm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "log.filter=debug",
        "--set",
        "log.file.max_file_size=1MiB",
        "--set",
        "log.export.max_size=4MiB",
        // The least that fits one sink and the export: (1 + 2) × 1MiB + 4MiB.
        "--set",
        "log.file.max_total_size=7MiB",
    ];
    let program = spawn(Path::new(BIN), &args);
    let addr = program.addr_of("echo");
    let dir = root.join("logs/udp-echo");
    let bound = 7 * MIB + MIB;

    let stop = Arc::new(AtomicBool::new(false));
    let found = Arc::new(Mutex::new(Found::default()));
    let sampler = std::thread::spawn({
        let (dir, found, stop) = (dir.clone(), Arc::clone(&found), Arc::clone(&stop));
        move || sample(&dir, bound, &found, &stop)
    });
    let flooders: Vec<_> = (1..=4)
        .map(|seed| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || flood(addr, &stop, seed))
        })
        .collect();
    // Until eight files have rolled and the budget has deleted some, or for two minutes under
    // an emulator.
    let deleted = |lowest| (2..u64::MAX).contains(&lowest);
    let started = Instant::now();
    loop {
        let found = found.lock().unwrap();
        let (lowest, rolled) = (found.lowest, found.highest);
        drop(found);
        if rolled >= 8 && deleted(lowest) || started.elapsed() > Duration::from_secs(120) {
            break;
        }
        assert!(!sampler.is_finished(), "the measurements stopped");
        std::thread::sleep(Duration::from_millis(20));
    }

    // An export of today's logs, while the storm goes on and the budget deletes what it chose.
    let exporting = Arc::new(AtomicBool::new(false));
    keep_sending(addr, b"export", &exporting);
    program.wait_for("log export started", &[]);
    exporting.store(true, Ordering::Relaxed);
    let finished = program.wait_for("log export finished", &[]);
    let field = |name: &str| finished.get(name).cloned().unwrap_or_default();
    let archive = PathBuf::from(field("export.path").as_str().unwrap_or_default());
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive).unwrap()).unwrap();
    let mut names = Vec::new();
    for index in 0..zip.len() {
        // Reading each entry checks its checksum.
        let mut entry = zip.by_index(index).unwrap();
        std::io::copy(&mut entry, &mut std::io::sink()).unwrap();
        names.push(entry.name().to_string());
    }
    let mut manifest = String::new();
    zip.by_name("manifest.txt")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    assert!(names.contains(&"udp-echo.log".to_string()), "{names:?}");
    let archived = std::fs::metadata(&archive).unwrap().len();
    assert!(archived <= 4 * MIB, "the archive holds {archived} bytes");

    stop.store(true, Ordering::Relaxed);
    flooders
        .into_iter()
        .for_each(|flooder| flooder.join().unwrap());
    sampler.join().unwrap();
    let took = started.elapsed();
    let found = Arc::try_unwrap(found).ok().unwrap().into_inner().unwrap();
    assert!(
        deleted(found.lowest) && found.export,
        "in {took:?}: {} rolled, below {} deleted, an export seen: {}",
        found.highest,
        found.lowest,
        found.export
    );

    // Stop it with a restart request, which works on every platform; datagrams may be lost
    // while the socket's buffer still holds the storm.
    let done = Arc::new(AtomicBool::new(false));
    keep_sending(addr, b"restart", &done);
    let exited = program.wait();
    done.store(true, Ordering::Relaxed);
    assert_eq!(exited.code, Some(75), "{}", exited.stderr);
    let (size, _, _, _) = measure(&dir);
    assert!(size <= bound, "after the exit: {size} bytes");
    // The flush barrier: the last line before the exit is in the main file.
    let main = std::fs::read_to_string(dir.join("udp-echo.log")).unwrap();
    let last = main.lines().last().unwrap_or_default();
    assert!(
        last.contains("rivium::process: stopped code=\"Restart\" exit_code=75"),
        "{last}"
    );
    let skipped = field("export.skipped");
    println!(
        "largest {} of {bound} bytes; {} files rolled, below {} deleted, in {took:?}; \
         an archive of {archived} bytes with {} files, {skipped} skipped\n{manifest}",
        found.largest,
        found.highest,
        found.lowest,
        names.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}
