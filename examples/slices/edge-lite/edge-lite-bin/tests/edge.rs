//! The business tests of edge-lite, through the process host: supervised units that panic and
//! fail, a last write bounded by the stop deadline, the API with the status envelope, a log file
//! of the units' own and its export, and a new configuration that a restart loads. Not built for
//! Android, where services run embedded.
#![cfg(not(target_os = "android"))]

use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rivium::log::Date;
use rivium_test::process::{Running, spawn};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_edge-lite");

/// A new, empty root directory for one test.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("edge-lite-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the program below `root` on a free port, with `args`.
fn start(root: &Path, args: &[&str]) -> (Running, SocketAddr) {
    let mut all = vec![
        "--root",
        root.to_str().unwrap(),
        "--set",
        "http.addr=127.0.0.1:0",
    ];
    all.extend(args);
    let program = spawn(Path::new(BIN), &all);
    let addr = program.addr_of("http");
    (program, addr)
}

/// Sends one HTTP/1.1 request and returns the status and the body.
fn send(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream
        .write_all(format!("{head}{body}").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    assert!(
        !head.contains("transfer-encoding"),
        "chunked replies are not read"
    );
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, raw[split + 4..].to_vec())
}

fn json(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let (status, body) = send(addr, method, path, body);
    let text = String::from_utf8_lossy(&body);
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{e}: {text}")),
    )
}

fn get(addr: SocketAddr, path: &str) -> Value {
    let (status, body) = json(addr, "GET", path, "");
    assert_eq!(status, 200, "{body}");
    body
}

/// Asks until `done` holds for the units, for at most 30 s; returns them.
fn units_until(addr: SocketAddr, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let units = get(addr, "/api/units").as_array().unwrap().clone();
        if done(&units) {
            return units;
        }
        assert!(
            Instant::now() < deadline,
            "the units never got there: {units:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn messages(events: &[serde_json::Map<String, Value>]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event["message"].as_str())
        .collect()
}

#[test]
fn units_poll_and_answer_with_the_status_envelope() {
    let root = root("units");
    let (program, addr) = start(&root, &[]);
    let units = units_until(addr, |units| {
        units.iter().all(|unit| unit["polls"].as_u64() > Some(0))
    });
    assert_eq!(units.len(), 2);
    assert_eq!(get(addr, "/api/units/1")["id"], 1);
    let missing = json(addr, "GET", "/api/units/9", "");
    let envelope = json!({"status": "error", "code": 404, "description": "no unit 9"});
    assert_eq!(missing, (404, envelope));
    #[cfg(unix)]
    {
        program.signal("TERM");
        let exited = program.wait();
        assert_eq!(exited.code, Some(0), "{}", exited.stderr);
        assert!(messages(&exited.events).contains(&"wrote the last readings"));
        let readings = std::fs::read_to_string(root.join("data/readings.jsonl")).unwrap();
        assert!(readings.lines().count() >= 2, "{readings}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_unit_that_panics_is_started_again_and_the_program_runs_on() {
    let root = root("panics");
    // Each run of a unit panics on its second poll: it never fails twice in a row.
    let (program, addr) = start(&root, &["--set", "faults.panic_every=2"]);
    let units = units_until(addr, |units| {
        units
            .iter()
            .all(|unit| unit["failures"].as_u64() >= Some(2))
    });
    assert!(
        units.iter().all(|unit| unit["state"] != "given up"),
        "{units:?}"
    );
    let failed = program.wait_for("a unit failed: it starts again", &[]);
    assert!(
        failed["why"]
            .as_str()
            .unwrap()
            .starts_with("panic: a panic injected in unit"),
        "{failed:?}"
    );
    #[cfg(unix)]
    {
        program.signal("TERM");
        let exited = program.wait();
        assert_eq!(exited.code, Some(0), "{}", exited.stderr);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn units_that_keep_failing_are_given_up_and_then_the_run_fails_with_70() {
    let root = root("given-up");
    let args = [
        "--set",
        "faults.error_every=1",
        "--set",
        "units.give_up_after=2",
    ];
    let (program, _) = start(&root, &args);
    let exited = program.wait();
    assert_eq!(exited.code, Some(70), "{}", exited.stderr);
    assert!(
        exited.stderr.contains("every unit has been given up"),
        "{}",
        exited.stderr
    );
    let events = messages(&exited.events);
    let given_up = events
        .iter()
        .filter(|m| m.ends_with("in a row: given up"))
        .count();
    assert_eq!(given_up, 2, "{events:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_last_write_ends_before_the_stop_deadline() {
    let root = root("deadline");
    // Readings pile up, and each one takes a second to write: more than the 3 s of the deadline.
    let args = [
        "--set",
        "store.every=1h",
        "--set",
        "faults.flush_delay_ms=1000",
    ];
    let (program, addr) = start(&root, &args);
    units_until(addr, |units| {
        units.iter().all(|unit| unit["polls"].as_u64() >= Some(4))
    });
    #[cfg(unix)]
    {
        let asked = Instant::now();
        program.signal("TERM");
        let exited = program.wait();
        let took = asked.elapsed();
        assert_eq!(exited.code, Some(0), "{}", exited.stderr);
        // The stop budget is 4 s: the stop timeout and what the host takes after it.
        assert!(took < Duration::from_millis(4500), "{took:?}");
        let cut = exited
            .events
            .iter()
            .find(|e| e["message"] == "the stop deadline cut the last write short");
        let cut = cut.unwrap_or_else(|| panic!("{:?}", messages(&exited.events)));
        let written = cut["written"].as_u64().unwrap();
        let readings =
            std::fs::read_to_string(root.join("data/readings.jsonl")).unwrap_or_default();
        assert_eq!(readings.lines().count() as u64, written);
        assert!(cut["left"].as_u64() > Some(0), "{cut:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_unit_events_have_a_log_file_of_their_own_which_the_api_exports() {
    let root = root("export");
    let units = r#"log.files=[{ name = "units", filter = "edge_lite::units=info" }]"#;
    let args = ["--set", units, "--set", "faults.error_every=2"];
    let (_program, addr) = start(&root, &args);
    units_until(addr, |units| {
        units
            .iter()
            .any(|unit| unit["failures"].as_u64() >= Some(1))
    });
    let mut files: Vec<String> =
        serde_json::from_value(get(addr, "/api/logs")["files"].clone()).unwrap();
    files.sort();
    assert_eq!(files, ["edge-lite", "units"]);
    let today = Date::today().to_string();
    let request = json!({"from": today, "to": today, "files": ["units"]}).to_string();
    let (status, created) = json(addr, "POST", "/api/logs/export", &request);
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_u64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while get(addr, &format!("/api/logs/export/{id}"))["state"] == "running" {
        assert!(Instant::now() < deadline, "the export did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        get(addr, &format!("/api/logs/export/{id}"))["state"],
        "done"
    );
    let (status, zip) = send(addr, "GET", &format!("/api/logs/export/{id}/file"), "");
    assert_eq!(status, 200);
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    let mut log = String::new();
    archive
        .by_name("units.log")
        .unwrap()
        .read_to_string(&mut log)
        .unwrap();
    assert!(log.contains("a unit failed: it starts again"), "{log}");
    assert!(!log.contains("listening"), "only the units' events: {log}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_new_configuration_is_checked_written_and_loaded_by_the_restart() {
    let root = root("config");
    let old = "[http]\naddr = \"127.0.0.1:0\"\n\n[units]\ncount = 1\n";
    std::fs::write(root.join("config.toml"), old).unwrap();
    let program = spawn(Path::new(BIN), &["--root", root.to_str().unwrap()]);
    let addr = program.addr_of("http");
    assert_eq!(get(addr, "/api/config")["units"]["count"], 1);

    // Every problem, and nothing written.
    let (status, refused) = json(
        addr,
        "PUT",
        "/api/config",
        "[units]\ncount = \"many\"\nspeed = 3\n",
    );
    assert_eq!(
        (status, &refused["status"]),
        (406, &json!("error")),
        "{refused}"
    );
    let problems = refused["description"].as_str().unwrap();
    assert_eq!(problems.lines().count(), 2, "{problems}");
    assert!(
        problems.contains("invalid configuration: units.count (file "),
        "{problems}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml")).unwrap(),
        old
    );

    // A valid one is written, with a backup, and the program exits for the restart that loads it.
    let new = "[http]\naddr = \"127.0.0.1:0\"\n\n[units]\ncount = 3\n";
    let (status, done) = json(addr, "PUT", "/api/config", new);
    assert_eq!(
        (status, done["status"].as_str()),
        (200, Some("success")),
        "{done}"
    );
    assert_eq!(program.wait().code, Some(75));
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml")).unwrap(),
        new
    );
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml.bak")).unwrap(),
        old
    );
    let program = spawn(Path::new(BIN), &["--root", root.to_str().unwrap()]);
    let addr = program.addr_of("http");
    assert_eq!(get(addr, "/api/config")["units"]["count"], 3);
    assert_eq!(units_until(addr, |units| units.len() == 3).len(), 3);
    drop(program);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn restarting_in_the_process_loads_the_new_configuration_too() {
    let root = root("in-process");
    let (program, addr) = start(&root, &["--set", "lifecycle.restart=in-process"]);
    // The overrides of the command line stay; the file is what the API replaces.
    let (status, _) = json(addr, "PUT", "/api/config", "[units]\ncount = 4\n");
    assert_eq!(status, 200);
    let addr = program.addr_of("http");
    assert_eq!(get(addr, "/api/config")["units"]["count"], 4);
    #[cfg(unix)]
    {
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
    let _ = std::fs::remove_dir_all(&root);
}
