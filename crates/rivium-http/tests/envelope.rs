//! The HTTP contract (V-8): every row of the status envelope, the mapping of error classes to
//! statuses, the errors of the framework, request ids in the response and the logs, the access
//! log and its observers, the probes and file downloads. The servers run in an embedded host in
//! this process, as a service would run them; one test checks everything in order, because a
//! process installs logging once.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use rivium::embedded::Host;
use rivium::error::{Class, Error, ErrorKind, ErrorType};
use rivium::{App, AppContext, Code, Health, HealthHandle, Result, Service};
use rivium_http::extract::{Json, Path as UrlPath, Query};
use rivium_http::{
    ApiResponse, ApiResult, HttpObserver, HttpServer, HttpSettings, ResponseInfo, file_response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The file that `/file` serves.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// The health item that `/readyz` reports.
static HEALTH: Mutex<Option<HealthHandle>> = Mutex::new(None);
/// What the observer of the `http` server saw.
static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

/// A response as the observer saw it: method, route, status, request id, error type.
type Seen = (String, Option<String>, u16, String, Option<String>);

struct Api;

#[derive(Serialize, Deserialize)]
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
        let error = info.error.map(|error| error.etype().name().to_string());
        let route = info.route.map(String::from);
        let seen = (
            info.method.to_string(),
            route,
            info.status.as_u16(),
            info.request_id.to_string(),
            error,
        );
        SEEN.lock().unwrap().push(seen);
    }
}

const TOKEN_EXPIRED: ErrorType =
    &ErrorKind::new("TokenExpired", Class::Unauthenticated).titled("token expired");

#[derive(Deserialize)]
struct Failure {
    #[serde(default)]
    upstream: bool,
    #[serde(default)]
    retry: bool,
    #[serde(default)]
    titled: bool,
}

#[derive(Serialize, Deserialize)]
struct Device {
    name: String,
    port: u16,
}

#[derive(Deserialize)]
struct Page {
    page: u32,
}

/// A handler failing with an error of `class`, whose context is a secret for server errors.
async fn failing(UrlPath(class): UrlPath<String>, Query(how): Query<Failure>) -> ApiResult {
    let class = match class.as_str() {
        "InvalidInput" => Class::InvalidInput,
        "InvalidBody" => Class::InvalidBody,
        "Unauthenticated" => Class::Unauthenticated,
        "Forbidden" => Class::Forbidden,
        "NotFound" => Class::NotFound,
        "Conflict" => Class::Conflict,
        "TooManyRequests" => Class::TooManyRequests,
        "Unavailable" => Class::Unavailable,
        "Timeout" => Class::Timeout,
        _ => Class::Internal,
    };
    let kind: ErrorType = match how.titled {
        true => TOKEN_EXPIRED,
        false => Box::leak(Box::new(ErrorKind::new("Failing", class))),
    };
    let mut error = Error::explain(kind, format!("secret detail of {class}"));
    error.set_retry(how.retry);
    Err(match how.upstream {
        true => error.into_up().into(),
        false => error.into(),
    })
}

async fn panicking() -> ApiResult {
    panic!("a handler panicked")
}

fn routes() -> Router {
    Router::new()
        .route("/ok", get(|| async { ApiResult::<()>::Ok(ApiResponse::Ok) }))
        .route(
            "/data",
            get(|| async { ApiResult::Ok(ApiResponse::Data(json!({"id": 7}))) }),
        )
        .route(
            "/created",
            post(|| async { ApiResult::Ok(ApiResponse::Created(json!({"id": 8}))) }),
        )
        .route("/error/{class}", get(failing))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route("/panic", get(panicking))
        .route(
            "/json",
            post(|Json(device): Json<Device>| async { ApiResult::Ok(ApiResponse::Data(device)) }),
        )
        .route(
            "/query",
            get(|Query(page): Query<Page>| async move { ApiResult::Ok(ApiResponse::Data(page.page)) }),
        )
        .route(
            "/path/{n}",
            get(|UrlPath(n): UrlPath<u32>| async move { ApiResult::Ok(ApiResponse::Data(n)) }),
        )
        .route(
            "/log",
            get(|| async {
                tracing::info!("inside the handler");
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route(
            "/teapot",
            get(|| async { (StatusCode::IM_A_TEAPOT, "short and stout").into_response() }),
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
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
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
fn the_http_contract() {
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

    // §2.1: success without data is the envelope; data is bare JSON.
    let ok = get_(http, "/ok");
    assert_eq!(
        (ok.status, ok.body.as_str()),
        (200, r#"{"status":"success","code":200,"description":""}"#)
    );
    assert_eq!(ok.header("content-type"), Some("application/json"));
    let data = get_(http, "/data");
    assert_eq!((data.status, data.body.as_str()), (200, r#"{"id":7}"#));
    let created = send(http, "POST", "/created", &[], "");
    assert_eq!(
        (created.status, created.body.as_str()),
        (201, r#"{"id":8}"#)
    );

    // §2.2: each class, its status, and what the caller is told.
    let cases: [(&str, u16, &str, &str); 14] = [
        ("InvalidInput", 400, "secret detail of InvalidInput", "INFO"),
        ("InvalidBody", 406, "secret detail of InvalidBody", "INFO"),
        ("Unauthenticated", 401, "unauthenticated", "INFO"),
        ("Unauthenticated?titled=true", 401, "token expired", "INFO"),
        ("Forbidden", 403, "forbidden", "INFO"),
        ("NotFound", 404, "secret detail of NotFound", "INFO"),
        ("Conflict", 409, "secret detail of Conflict", "INFO"),
        (
            "TooManyRequests",
            429,
            "secret detail of TooManyRequests",
            "WARN",
        ),
        (
            "Unavailable",
            503,
            "service unavailable (request_id=",
            "WARN",
        ),
        ("Timeout", 504, "timeout (request_id=", "WARN"),
        ("Internal", 500, "internal error (request_id=", "ERROR"),
        (
            "NotFound?upstream=true",
            500,
            "internal error (request_id=",
            "ERROR",
        ),
        (
            "TooManyRequests?upstream=true",
            503,
            "service unavailable (request_id=",
            "WARN",
        ),
        ("Timeout?upstream=true", 504, "timeout (request_id=", "WARN"),
    ];
    for (path, status, description, level) in cases {
        let reply = get_(http, &format!("/error/{path}"));
        let (outcome, code, said) = reply.envelope();
        assert_eq!((outcome.as_str(), code), ("error", status), "{path}");
        let id = reply.request_id().to_string();
        match description.ends_with('=') {
            true => assert_eq!(said, format!("{description}{id})"), "{path}"),
            false => assert_eq!(said, description, "{path}"),
        }
        assert_eq!(reply.header("retry-after"), None, "{path}");
        let line = access_line(&root, &id);
        assert!(line.contains(&format!(" {level} ")), "{path}: {line}");
        assert!(
            line.contains("error.type=\"Failing\"") || path.contains("titled"),
            "{line}"
        );
        assert!(line.contains("error.context=\"secret detail of"), "{line}");
    }
    for path in [
        "TooManyRequests?",
        "Unavailable?",
        "TooManyRequests?upstream=true&",
    ] {
        let reply = get_(http, &format!("/error/{path}retry=true"));
        assert_eq!(reply.header("retry-after"), Some("1"), "{path}");
    }
    let reply = get_(http, "/error/Timeout?retry=true");
    assert_eq!(
        reply.header("retry-after"),
        None,
        "only 429 and 503 carry Retry-After"
    );
    let exposed = get_(admin, "/error/Internal");
    assert_eq!(
        exposed.envelope().2,
        "secret detail of Internal",
        "expose_internal_detail"
    );
    let unobserved = exposed.request_id().to_string();

    // §2.3: errors of the framework.
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
    let json_type = [("content-type", "application/json")];
    let large = send(
        http,
        "POST",
        "/json",
        &json_type,
        &format!("{{\"name\":\"{}\"}}", "x".repeat(80)),
    );
    assert_eq!(
        large.envelope(),
        ("error".into(), 413, "Payload Too Large".into())
    );
    let device = send(
        http,
        "POST",
        "/json",
        &json_type,
        r#"{"name":"printer","port":9100}"#,
    );
    assert_eq!(
        (device.status, device.body.as_str()),
        (200, r#"{"name":"printer","port":9100}"#)
    );
    let bad = send(
        http,
        "POST",
        "/json",
        &json_type,
        r#"{"name":"printer","port":"x"}"#,
    );
    let (_, code, said) = bad.envelope();
    assert_eq!(code, 406);
    assert!(
        said.starts_with("port: invalid type: string \"x\", expected u16 at line 1 column"),
        "{said}"
    );
    let missing = send(http, "POST", "/json", &json_type, r#"{"name":"printer"}"#);
    assert!(
        missing
            .envelope()
            .2
            .starts_with("missing field `port` at line 1"),
        "{}",
        missing.body
    );
    let trailing = send(
        http,
        "POST",
        "/json",
        &json_type,
        r#"{"name":"p","port":1} x"#,
    );
    assert!(
        trailing.envelope().2.starts_with("trailing characters"),
        "{}",
        trailing.body
    );
    let untyped = send(
        http,
        "POST",
        "/json",
        &[],
        r#"{"name":"printer","port":9100}"#,
    );
    let said = untyped.envelope();
    assert_eq!(
        (said.1, said.2.as_str()),
        (
            406,
            "expected a JSON body, with Content-Type: application/json"
        )
    );
    let query = get_(http, "/query?page=two");
    let (_, code, said) = query.envelope();
    assert_eq!(code, 400);
    assert!(
        said.contains("page: invalid digit found in string"),
        "{said}"
    );
    let path = get_(http, "/path/seven");
    let (_, code, said) = path.envelope();
    assert_eq!(code, 400);
    assert!(said.contains("Cannot parse `seven` to a `u32`"), "{said}");
    let slow = get_(http, "/slow");
    let id = slow.request_id().to_string();
    assert_eq!(
        slow.envelope().2,
        format!("service unavailable (request_id={id})")
    );
    assert_eq!((slow.status, slow.header("retry-after")), (503, Some("1")));
    assert!(access_line(&root, &id).contains("error.type=\"RequestTimedOut\""));
    let panicked = get_(http, "/panic");
    let id = panicked.request_id().to_string();
    assert_eq!(
        panicked.envelope(),
        (
            "error".into(),
            500,
            format!("internal error (request_id={id})")
        )
    );
    assert!(log(&root).contains("panic.message=\"a handler panicked\""));
    let teapot = get_(http, "/teapot");
    assert_eq!(
        teapot.envelope(),
        ("error".into(), 418, "I'm a teapot".into())
    );

    // §2.4: request ids, in the response and in every line the request logs.
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
    for invalid in ["has space", &"x".repeat(129)] {
        let reply = send(http, "GET", "/ok", &[("x-request-id", invalid)], "");
        let id = reply.request_id();
        assert!(
            id.len() == 36 && id.as_bytes()[14] == b'7',
            "a UUIDv7, not {id}"
        );
    }
    let seen = SEEN.lock().unwrap();
    let last = seen.iter().find(|seen| seen.3 == "abc-123").unwrap();
    assert_eq!(last.0, "GET");
    assert_eq!(
        (last.1.as_deref(), last.2, last.4.as_deref()),
        (Some("/log"), 200, None)
    );
    let failed = seen.iter().find(|seen| seen.2 == 409).unwrap();
    assert_eq!(
        (failed.1.as_deref(), failed.4.as_deref()),
        (Some("/error/{class}"), Some("Failing"))
    );
    let unrouted = seen.iter().find(|seen| seen.2 == 404 && seen.1.is_none());
    assert!(unrouted.is_some(), "an unknown route has no route");
    assert!(
        !seen.iter().any(|seen| seen.3 == unobserved),
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
