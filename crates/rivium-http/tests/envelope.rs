//! `HttpServer` over real connections: its settings reach the contract layer, responses carry
//! request ids and the access log is written with them, each server tells only its own
//! observers, the probes answer, and files download with ranges. The contract itself is checked
//! row by row on the contract layer (`layer.rs`). The servers run in an embedded host in this
//! process, as a service would run them; one test checks everything in order, because a process
//! installs logging once.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::routing::{get, post};
use rivium::embedded::Host;
use rivium::error::{Error, kinds};
use rivium::{App, AppContext, Code, Health, HealthHandle, Result, Service};
use rivium_http::extract::Json;
use rivium_http::{
    ApiResponse, ApiResult, HttpObserver, HttpServer, HttpSettings, ResponseInfo, file_response,
};
use serde_json::{Value, json};

/// The file that `/file` serves.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// The health item that `/readyz` reports.
static HEALTH: Mutex<Option<HealthHandle>> = Mutex::new(None);
/// The request ids that the observer of the `http` server saw.
static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Api;

#[derive(serde::Serialize, serde::Deserialize)]
struct Config {
    http: HttpSettings,
    admin: HttpSettings,
}

impl Default for Config {
    fn default() -> Self {
        let mut http = HttpSettings::default();
        http.addr = SocketAddr::from(([127, 0, 0, 1], 0));
        http.request_timeout = Duration::from_secs(1);
        http.body_limit = 64;
        let mut admin = http.clone();
        (admin.probes, admin.expose_internal_detail) = (false, true);
        Config { http, admin }
    }
}

impl App for Api {
    const NAME: &'static str = "api";
    const VERSION: &'static str = "1.0.0";
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        *HEALTH.lock().unwrap() = Some(ctx.health().register("store"));
        let http = HttpServer::new(&config.http, routes(), ctx).observe(Arc::new(Observer));
        let admin = HttpServer::new(&config.admin, routes(), ctx).named("admin");
        Ok(vec![Box::new(http), Box::new(admin)])
    }
}

struct Observer;

impl HttpObserver for Observer {
    fn on_response(&self, info: &ResponseInfo<'_>) {
        SEEN.lock().unwrap().push(info.request_id.to_string());
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Device {
    name: String,
}

fn routes() -> Router {
    Router::new()
        .route(
            "/ok",
            get(|| async { ApiResult::<()>::Ok(ApiResponse::Ok) }),
        )
        .route(
            "/missing-device",
            get(|| async {
                ApiResult::<()>::Err(Error::explain(kinds::NOT_FOUND, "no device 7").into())
            }),
        )
        .route(
            "/broken",
            get(|| async {
                ApiResult::<()>::Err(Error::explain(kinds::INTERNAL, "secret detail").into())
            }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route(
            "/json",
            post(|Json(device): Json<Device>| async { ApiResult::Ok(ApiResponse::Data(device)) }),
        )
        .route(
            "/log",
            get(|| async {
                tracing::info!("inside the handler");
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route(
            "/file",
            get(|request: Request| async move {
                file_response(request, FILE.get().unwrap(), "report 2026-10-08.txt").await
            }),
        )
        .route(
            "/missing",
            get(|request: Request| async move {
                file_response(request, Path::new("/no/such/file"), "missing.txt").await
            }),
        )
}

/// A response, as read off the wire.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(header, _)| header == name);
        found.map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    /// The envelope's status, code and description.
    fn envelope(&self) -> (String, u16, String) {
        let json = self.json();
        let keys = json.as_object().unwrap().keys();
        assert_eq!(
            keys.len(),
            3,
            "the envelope has three fields: {}",
            self.body
        );
        let text = |key: &str| json[key].as_str().unwrap().to_string();
        let code = json["code"].as_u64().unwrap() as u16;
        assert_eq!(code, self.status, "{}", self.body);
        (text("status"), code, text("description"))
    }

    fn request_id(&self) -> &str {
        self.header("x-request-id")
            .expect("every response has x-request-id")
    }
}

/// Sends one HTTP/1.1 request and reads the whole response.
fn send(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n");
    head += &format!("Content-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head += &format!("{name}: {value}\r\n");
    }
    stream
        .write_all(format!("{head}\r\n{body}").as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
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
        body: body.to_string(),
    };
    assert!(
        reply.header("transfer-encoding").is_none(),
        "chunked replies are not read"
    );
    reply
}

fn get_(addr: SocketAddr, path: &str) -> Reply {
    send(addr, "GET", path, &[], "")
}

/// The main log file, once every line logged so far is in it.
fn log(root: &Path) -> String {
    rivium::log::flush(Duration::from_secs(10)).unwrap();
    std::fs::read_to_string(root.join("logs/api/api.log")).unwrap()
}

/// The address the server `name` listens on, from its `listening` event.
fn addr_of(root: &Path, name: &str) -> SocketAddr {
    let log = log(root);
    let marker = format!("listening service.name={name} listen.addr=");
    let line = log
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("{log}"));
    let addr = line
        .split(&marker)
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    addr.parse().unwrap()
}

/// The access log line of the request `id`.
fn access_line(root: &Path, id: &str) -> String {
    let log = log(root);
    let needle = format!("request_id={id}");
    let mut lines = log
        .lines()
        .filter(|line| line.contains("rivium_http::access: request "));
    let line = lines.find(|line| line.contains(&needle));
    line.unwrap_or_else(|| panic!("no access line for {id} in\n{log}"))
        .to_string()
}

/// Rivium's panic hook writes panics to the log once logging is installed: this test's own
/// failures go to standard error as well.
fn failures_to_stderr() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
}

#[test]
fn http_servers_keep_the_contract_over_connections() {
    let root = std::env::temp_dir().join(format!("rivium-http-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("report.txt");
    std::fs::write(&file, "0123456789").unwrap();
    FILE.set(file).unwrap();
    let host = Host::new::<Api>();
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "log.filter=info,rivium_http=debug",
    ];
    assert_eq!(
        host.start(&args.map(String::from)),
        Code::Ok,
        "{:?}",
        host.last_error()
    );
    failures_to_stderr();
    let (http, admin) = (addr_of(&root, "http"), addr_of(&root, "admin"));

    // The settings of each server reach its contract layer: the request timeout, the body
    // limit and whether server errors show their detail.
    let slow = get_(http, "/slow");
    let id = slow.request_id().to_string();
    let expected = format!("service unavailable (request_id={id})");
    assert_eq!(slow.envelope(), ("error".into(), 503, expected));
    assert_eq!(slow.header("retry-after"), Some("1"));
    assert!(access_line(&root, &id).contains("error.type=\"RequestTimedOut\""));
    let json_type = [("content-type", "application/json")];
    let large = format!("{{\"name\":\"{}\"}}", "x".repeat(80));
    let large = send(http, "POST", "/json", &json_type, &large);
    assert_eq!(
        large.envelope(),
        ("error".into(), 413, "Payload Too Large".into())
    );
    let small = send(http, "POST", "/json", &json_type, r#"{"name":"printer"}"#);
    assert_eq!(
        (small.status, small.body.as_str()),
        (200, r#"{"name":"printer"}"#)
    );
    let hidden = get_(http, "/broken");
    let id = hidden.request_id().to_string();
    let expected = format!("internal error (request_id={id})");
    assert_eq!(hidden.envelope(), ("error".into(), 500, expected));
    let line = access_line(&root, &id);
    assert!(
        line.contains(" ERROR ") && line.contains("error.context=\"secret detail\""),
        "{line}"
    );
    let exposed = get_(admin, "/broken");
    assert_eq!(
        exposed.envelope().2,
        "secret detail",
        "expose_internal_detail"
    );
    let unobserved = exposed.request_id().to_string();

    // Answers through the envelope, with request ids in the response and in every line the
    // request logs.
    let ok = get_(http, "/ok");
    assert_eq!(
        (ok.status, ok.body.as_str()),
        (200, r#"{"status":"success","code":200,"description":""}"#)
    );
    let missing = get_(http, "/missing-device");
    assert_eq!(
        missing.envelope(),
        ("error".into(), 404, "no device 7".into())
    );
    let unmatched = get_(http, "/nope");
    assert_eq!(
        unmatched.envelope(),
        ("error".into(), 404, "Not Found".into())
    );
    let method = send(http, "DELETE", "/ok", &[], "");
    assert_eq!(
        method.envelope(),
        ("error".into(), 405, "Method Not Allowed".into())
    );
    assert_eq!(method.header("allow"), Some("GET,HEAD"));
    let given = send(http, "GET", "/log", &[("x-request-id", "abc-123")], "");
    assert_eq!(given.request_id(), "abc-123");
    let log_text = log(&root);
    assert!(
        log_text.contains("request{request_id=abc-123}: "),
        "{log_text}"
    );
    let line = access_line(&root, "abc-123");
    for field in [
        "http.method=GET",
        "http.route=\"/log\"",
        "http.status=200",
        "latency_ms=",
    ] {
        assert!(line.contains(field), "{field} in {line}");
    }
    assert!(
        line.contains(" INFO ") && !line.contains("error."),
        "{line}"
    );
    let generated = get_(http, "/ok");
    let id = generated.request_id();
    assert!(
        id.len() == 36 && id.as_bytes()[14] == b'7',
        "a UUIDv7, not {id}"
    );

    // Each server tells its own observers only.
    let seen = SEEN.lock().unwrap();
    assert!(
        seen.iter().any(|seen| seen == "abc-123"),
        "the observer sees its server"
    );
    assert!(
        !seen.contains(&unobserved),
        "only the observed server reports"
    );
    drop(seen);

    // Probes.
    let live = get_(http, "/livez");
    assert_eq!(
        (live.status, live.body.as_str()),
        (200, r#"{"status":"live"}"#)
    );
    assert!(live.header("x-request-id").is_some());
    let ready = get_(http, "/readyz");
    assert_eq!(ready.status, 200);
    assert_eq!(
        ready.json(),
        json!({"status": "ready", "phase": "running", "checks": {"store": "healthy"}})
    );
    let probe_line = access_line(&root, ready.request_id());
    assert!(probe_line.contains(" DEBUG "), "{probe_line}");
    HEALTH
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .set(Health::Unhealthy("disk full".into()));
    let unready = get_(http, "/readyz");
    assert_eq!(unready.status, 503);
    assert_eq!(
        unready.json(),
        json!({"status": "not ready", "phase": "running", "checks": {"store": "unhealthy"}})
    );
    assert_eq!(get_(admin, "/livez").envelope().1, 404, "probes off");

    // Downloads.
    let whole = get_(http, "/file");
    assert_eq!((whole.status, whole.body.as_str()), (200, "0123456789"));
    assert_eq!(
        whole.header("content-disposition"),
        Some("attachment; filename=\"report 2026-10-08.txt\"")
    );
    let part = send(http, "GET", "/file", &[("range", "bytes=2-5")], "");
    assert_eq!((part.status, part.body.as_str()), (206, "2345"));
    assert_eq!(part.header("content-range"), Some("bytes 2-5/10"));
    let beyond = send(http, "GET", "/file", &[("range", "bytes=20-30")], "");
    assert_eq!(beyond.envelope().1, 416);
    assert_eq!(
        get_(http, "/missing").envelope(),
        ("error".into(), 404, "Not Found".into())
    );

    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    let _ = std::fs::remove_dir_all(&root);
}
