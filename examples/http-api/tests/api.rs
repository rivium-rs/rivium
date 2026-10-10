//! The program as its clients use it: the device registry with the status envelope, the probes,
//! request ids from `traceparent`, and log export over HTTP from the first request to the
//! download. Not built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rivium::log::Date;
use rivium_test::process::{Running, spawn};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_http-api");

/// A new, empty root directory for one test.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("http-api-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the program below `root`, with the access log in a file of its own.
fn start(root: &Path, args: &[&str]) -> (Running, SocketAddr) {
    let access = r#"log.files=[{ name = "access", filter = "rivium_http::access=info" }]"#;
    let mut all = vec!["--root", root.to_str().unwrap(), "--set", access];
    all.extend(args);
    let program = spawn(Path::new(BIN), &all);
    let addr = program.addr_of("http");
    (program, addr)
}

/// A response, as read off the wire.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(header, _)| header == name);
        found.map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        let text = String::from_utf8_lossy(&self.body);
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {text}"))
    }

    /// The envelope's code and description.
    fn error(&self) -> (u16, String) {
        let json = self.json();
        assert_eq!(json["status"], "error", "{json}");
        let description = json["description"].as_str().unwrap().to_string();
        (json["code"].as_u64().unwrap() as u16, description)
    }
}

/// Sends one HTTP/1.1 request, with a JSON body and another header when there are, and reads the
/// response.
fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    header: Option<(&str, &str)>,
) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let body = body.unwrap_or_default();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n");
    head += &format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some((name, value)) = header {
        head += &format!("{name}: {value}\r\n");
    }
    stream
        .write_all(format!("{head}\r\n{body}").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect();
    let reply = Reply {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    };
    assert!(
        reply.header("transfer-encoding").is_none(),
        "chunked replies are not read"
    );
    reply
}

fn get(addr: SocketAddr, path: &str) -> Reply {
    send(addr, "GET", path, None, None)
}

fn post(addr: SocketAddr, path: &str, body: &str) -> Reply {
    send(addr, "POST", path, Some(body), None)
}

/// Waits until the export `id` is no longer running, and returns its progress.
fn finished(addr: SocketAddr, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let progress = get(addr, &format!("/api/logs/exports/{id}")).json();
        if progress["state"] != "running" || Instant::now() > deadline {
            return progress;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn devices_answer_with_the_status_envelope() {
    let root = root("devices");
    let (_program, addr) = start(&root, &[]);
    let printer = r#"{"name":"printer","kind":"label"}"#;
    let created = post(addr, "/api/devices", printer);
    assert_eq!(
        (created.status, created.json()),
        (201, json!({"id": 1, "name": "printer", "kind": "label"}))
    );
    post(addr, "/api/devices", r#"{"name":"scale","kind":"weight"}"#);
    let one = get(addr, "/api/devices/1");
    assert_eq!(
        (one.status, one.json()["name"].clone()),
        (200, json!("printer"))
    );
    let labels = get(addr, "/api/devices?kind=label").json();
    assert_eq!(labels.as_array().map(Vec::len), Some(1), "{labels}");
    assert_eq!(
        get(addr, "/api/devices/9").error(),
        (404, "no device 9".to_string())
    );
    let taken = post(addr, "/api/devices", printer).error();
    assert_eq!(
        taken,
        (409, "a device is named \"printer\" already".to_string())
    );
    let empty = post(addr, "/api/devices", r#"{"name":" ","kind":"label"}"#).error();
    assert_eq!(empty, (406, "name: must not be empty".to_string()));
    let (code, said) = post(addr, "/api/devices", r#"{"name":"x"}"#).error();
    assert!(
        code == 406 && said.starts_with("missing field `kind`"),
        "{said}"
    );
    assert_eq!(get(addr, "/api/devices/one").error().0, 400);
    let removed = send(addr, "DELETE", "/api/devices/1", None, None);
    assert_eq!(
        removed.body,
        br#"{"status":"success","code":200,"description":""}"#
    );
    assert_eq!(get(addr, "/api/devices/1").error().0, 404);
    assert_eq!(get(addr, "/livez").status, 200);
    assert_eq!(get(addr, "/readyz").json()["status"], "ready");

    // A client's `traceparent` sets the request id; without one, the server makes one.
    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let traced = send(
        addr,
        "GET",
        "/api/devices",
        None,
        Some(("traceparent", traceparent)),
    );
    let trace = Some("4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(traced.header("x-request-id"), trace);
    let made = get(addr, "/api/devices");
    assert!(made.header("x-request-id").is_some_and(|id| id.len() == 36));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn logs_export_over_http_from_the_request_to_the_download() {
    let root = root("export");
    let (program, addr) = start(&root, &["--set", "log.export.expire_after=2s"]);
    let kinds = get(addr, "/api/logs/kinds").json();
    assert_eq!(kinds, json!([{"kind": "service"}, {"kind": "access"}]));
    post(addr, "/api/devices", r#"{"name":"printer","kind":"label"}"#);

    let today = Date::today();
    let request = json!({"from": today, "to": today, "kinds": ["service", "access"]}).to_string();
    let started = post(addr, "/api/logs/exports", &request);
    assert_eq!(started.status, 201);
    let id = started.json()["id"].as_u64().unwrap();
    let progress = finished(addr, id);
    assert_eq!(progress, json!({"percent": 100, "state": "done"}));

    // The archive, as often as asked, and in ranges.
    let archive = format!("/api/logs/exports/{id}/archive");
    let whole = get(addr, &archive);
    assert_eq!(whole.status, 200);
    let name = format!("attachment; filename=\"http-api-logs-{today}_{today}.zip\"");
    assert_eq!(whole.header("content-disposition"), Some(name.as_str()));
    let mut zip = zip::ZipArchive::new(Cursor::new(whole.body.clone())).unwrap();
    let names: Vec<String> = zip.file_names().map(String::from).collect();
    for expected in ["http-api.log", "access.log", "manifest.txt"] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} in {names:?}"
        );
    }
    let mut access = String::new();
    zip.by_name("access.log")
        .unwrap()
        .read_to_string(&mut access)
        .unwrap();
    assert!(
        access.contains("http.route=\"/api/devices\"") && access.contains("http.status=201"),
        "{access}"
    );
    let again = send(addr, "GET", &archive, None, Some(("Range", "bytes=0-3")));
    assert_eq!(
        (again.status, again.body.as_slice()),
        (206, b"PK\x03\x04".as_slice())
    );

    // What the binding refuses: kinds it does not know in the clients' own format, the rest
    // with the status of the error.
    let unknown = json!({"from": today, "to": today, "kinds": ["audit", "service"]});
    let unknown = post(addr, "/api/logs/exports", &unknown.to_string());
    let answer = json!({"unknown": ["audit"], "kinds": ["service", "access"]});
    assert_eq!((unknown.status, unknown.json()), (400, answer));
    let refused = [
        (
            json!({"from": "2020-01-01", "to": "2020-01-02"}),
            404,
            "no log files of [\"http-api\", \"access\"] from 2020-01-01 to 2020-01-02",
        ),
        (
            json!({"from": "2026-10-02", "to": "2026-10-01"}),
            400,
            "the first day 2026-10-02 is after the last day 2026-10-01",
        ),
    ];
    for (body, code, said) in refused {
        assert_eq!(
            post(addr, "/api/logs/exports", &body.to_string()).error(),
            (code, said.to_string())
        );
    }
    let (code, said) = post(
        addr,
        "/api/logs/exports",
        r#"{"from":"yesterday","to":"2026-10-01"}"#,
    )
    .error();
    assert!(
        code == 406 && said.starts_with("from: not a day written like 2026-10-08"),
        "{said}"
    );

    // Cancelling removes the archive; a finished one expires.
    let cancelled = send(
        addr,
        "DELETE",
        &format!("/api/logs/exports/{id}"),
        None,
        None,
    );
    assert_eq!(cancelled.status, 200);
    assert_eq!(
        get(addr, &format!("/api/logs/exports/{id}")).json()["state"],
        "cancelled"
    );
    assert_eq!(
        get(addr, &archive).error(),
        (404, format!("log export {id} was cancelled"))
    );
    let id = post(addr, "/api/logs/exports", &request).json()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(finished(addr, id)["state"], "done");
    std::thread::sleep(Duration::from_millis(2_500));
    let gone = get(addr, &format!("/api/logs/exports/{id}")).error();
    assert_eq!(
        gone,
        (
            404,
            format!("no log export {id}: it was replaced or expired")
        )
    );
    let exports = std::fs::read_dir(root.join("logs/http-api")).unwrap();
    let left = exports.filter(|entry| {
        let name = entry.as_ref().unwrap().file_name();
        name.to_string_lossy().starts_with("export-")
    });
    assert_eq!(left.count(), 0);
    #[cfg(unix)]
    {
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
    let _ = std::fs::remove_dir_all(&root);
}
