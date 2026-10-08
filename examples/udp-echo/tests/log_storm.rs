//! A storm of log lines from the real program (S-8, V-14): datagrams of random text from four
//! threads, each logged at `debug`, while the log directory is measured every 20ms. It stays
//! within the budget plus one file per sink while the budget deletes rolled files, and the
//! program's last line is in the file once it exits. Not built for Android, where services run
//! embedded.
#![cfg(not(target_os = "android"))]

use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rivium_test::process::spawn;

const BIN: &str = env!("CARGO_BIN_EXE_udp-echo");
const MIB: u64 = 1 << 20;

/// The size of the files in `dir`, and the lowest and highest sequence numbers of the rolled
/// files.
fn measure(dir: &Path) -> (u64, u64, u64) {
    let (mut size, mut lowest, mut highest) = (0, u64::MAX, 0);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        // A file deleted while it is measured counts as gone.
        size += entry.metadata().map_or(0, |meta| meta.len());
        let name = entry.file_name().to_string_lossy().into_owned();
        // `udp-echo.<date>.<N>.log` and its archive `….log.gz`.
        let parts: Vec<&str> = name.split('.').collect();
        if let [_, _, n, "log", ..] = parts[..]
            && let Ok(n) = n.parse()
        {
            (lowest, highest) = (lowest.min(n), highest.max(n));
        }
    }
    (size, lowest, highest)
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

#[test]
fn a_log_storm_stays_within_the_budget_and_the_last_line_reaches_the_file() {
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
        // The least that fits one sink: (1 + 2) × 1MiB.
        "--set",
        "log.file.max_total_size=3MiB",
    ];
    let program = spawn(Path::new(BIN), &args);
    let addr = program.addr_of("echo");
    let dir = root.join("logs/udp-echo");
    let bound = 3 * MIB + MIB;

    let stop = Arc::new(AtomicBool::new(false));
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
    let (mut largest, mut lowest, mut rolled) = (0, u64::MAX, 0);
    while !(rolled >= 8 && deleted(lowest)) && started.elapsed() < Duration::from_secs(120) {
        let (size, low, high) = measure(&dir);
        assert!(
            size <= bound,
            "the log directory holds {size} bytes, more than {bound}"
        );
        (largest, lowest, rolled) = (largest.max(size), low, high);
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::Relaxed);
    flooders
        .into_iter()
        .for_each(|flooder| flooder.join().unwrap());
    let took = started.elapsed();
    assert!(
        deleted(lowest),
        "no rolled file was deleted in {took:?}; {rolled} rolled"
    );

    // Stop it with a restart request, which works on every platform; datagrams may be lost
    // while the socket's buffer still holds the storm.
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let asking = std::thread::spawn({
        let done = Arc::clone(&done);
        move || {
            while !done.load(Ordering::Relaxed) {
                let _ = socket.send_to(b"restart", addr);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    let exited = program.wait();
    done.store(true, Ordering::Relaxed);
    asking.join().unwrap();
    assert_eq!(exited.code, Some(75), "{}", exited.stderr);
    let (size, _, _) = measure(&dir);
    assert!(size <= bound, "after the exit: {size} bytes");
    // The flush barrier: the last line before the exit is in the main file.
    let main = std::fs::read_to_string(dir.join("udp-echo.log")).unwrap();
    let last = main.lines().last().unwrap_or_default();
    assert!(
        last.contains("rivium::process: stopped exit_code=75"),
        "{last}"
    );
    println!(
        "largest {largest} of {bound} bytes; {rolled} files rolled, below {lowest} deleted, in {took:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
