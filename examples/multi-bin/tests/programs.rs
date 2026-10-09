//! The programs as a supervisor runs them: each keeps the lifecycle contract (S-1), and the
//! collector reads the station. Not built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

use std::path::{Path, PathBuf};

use rivium_test::process::{lifecycle_contract, spawn};

const STATION: &str = env!("CARGO_BIN_EXE_station");
const COLLECTOR: &str = env!("CARGO_BIN_EXE_collector");

/// A new, empty root directory for one test.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("multi-bin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_station_keeps_the_lifecycle_contract() {
    lifecycle_contract(Path::new(STATION), &[]);
}

#[test]
fn the_collector_keeps_the_lifecycle_contract() {
    lifecycle_contract(Path::new(COLLECTOR), &[]);
}

#[test]
fn the_collector_reads_the_station() {
    // Both programs below one root: each logs below logs/<its name>.
    let root = root("read");
    let rooted = ["--root", root.to_str().unwrap()];
    let station = spawn(Path::new(STATION), &rooted);
    let addr = station.addr_of("query").to_string();
    let stations = format!("collect.stations=[\"{addr}\"]");
    let args = [rooted[0], rooted[1], "--set", &stations];
    let collector = spawn(Path::new(COLLECTOR), &args);
    let reading = collector.wait_for("reading", &[("station", &addr)]);
    let value = reading["value"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((20.0..30.0).contains(&value), "{reading:?}");
    #[cfg(unix)]
    for program in [collector, station] {
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
}
