//! The checks of the HTTP contract that every way of keeping it must pass: the status envelope,
//! the statuses and descriptions of errors, the errors of the framework, `NoEnvelope`, request
//! ids and the access log. The contract layer's test and the test of a middleware made of the
//! public parts alone run them; requests go through `oneshot`, without a server.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use rivium::error::{Class, Error, ErrorKind, ErrorType, kinds};
use rivium_http::extract::{Json, Path, Query};
use rivium_http::{ApiError, ApiResponse, ApiResult, NoEnvelope, RequestId};
use rivium_test::LogCapture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

/// How long a handler may take.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(1);
/// The largest request body.
pub(crate) const BODY_LIMIT: u64 = 64;

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
async fn failing(Path(class): Path<String>, Query(how): Query<Failure>) -> ApiResult {
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

/// The routes under test, among them a service's own `/livez` and `/readyz`.
pub(crate) fn routes() -> Router {
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
            get(|Path(n): Path<u32>| async move { ApiResult::Ok(ApiResponse::Data(n)) }),
        )
        .route(
            "/log",
            get(|| async {
                tracing::info!("inside the handler");
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route(
            "/id",
            get(|id: RequestId| async move { ApiResult::Ok(ApiResponse::Data(id.as_str().to_string())) }),
        )
        .route(
            "/teapot",
            get(|| async { (StatusCode::IM_A_TEAPOT, "short and stout") }),
        )
        .route(
            "/gateway",
            get(|| async { (StatusCode::BAD_GATEWAY, "upstream down") }),
        )
        .route(
            "/own",
            get(|| async {
                let body = r#"{"failedArr":[3]}"#;
                (StatusCode::CONFLICT, NoEnvelope, [("x-own", "kept")], body)
            }),
        )
        .route(
            "/own-failed",
            get(|| async {
                let error = Error::explain(kinds::CONFLICT, "device 3 is taken");
                (StatusCode::CONFLICT, NoEnvelope, ApiError(error))
            }),
        )
        .route("/livez", get(|| async { "live" }))
        .route(
            "/readyz",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, NoEnvelope, "not ready") }),
        )
}

/// A response, read whole.
pub(crate) struct Reply {
    pub(crate) status: u16,
    headers: HeaderMap,
    pub(crate) body: String,
}

impl Reply {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The envelope's status, code and description.
    pub(crate) fn envelope(&self) -> (String, u16, String) {
        let json: Value =
            serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body));
        let keys = json.as_object().unwrap().keys();
        assert_eq!(
            keys.len(),
            3,
            "the envelope has three fields: {}",
            self.body
        );
        let order = ["\"status\":", "\"code\":", "\"description\":"].map(|key| self.body.find(key));
        let in_order = order[0] == Some(1) && order[0] < order[1] && order[1] < order[2];
        assert!(in_order, "status, code, description: {}", self.body);
        let text = |key: &str| json[key].as_str().unwrap().to_string();
        let code = json["code"].as_u64().unwrap() as u16;
        assert_eq!(code, self.status, "{}", self.body);
        (text("status"), code, text("description"))
    }

    pub(crate) fn request_id(&self) -> &str {
        self.header("x-request-id")
            .expect("every response has x-request-id")
    }
}

/// Sends one request through `app` and reads the whole response.
pub(crate) async fn send(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::from(body.to_string())).unwrap();
    let Ok(response) = app.clone().oneshot(request).await;
    let (parts, body) = response.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

pub(crate) async fn get_(app: &Router, path: &str) -> Reply {
    send(app, "GET", path, &[], "").await
}

/// The access log line of the request `id`: there is exactly one.
pub(crate) fn access_line(logs: &LogCapture, id: &str) -> Map<String, Value> {
    let mut lines = logs.with_message("request");
    lines.retain(|line| line.get("request_id").and_then(Value::as_str) == Some(id));
    assert_eq!(lines.len(), 1, "one access line for {id}: {lines:?}");
    lines.remove(0)
}

fn text<'a>(line: &'a Map<String, Value>, key: &str) -> &'a str {
    line.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The access log line of `reply`'s request, which logged an error only there, and only once.
fn failed_line(logs: &LogCapture, reply: &Reply) -> Map<String, Value> {
    let id = reply.request_id();
    let errors = logs.events().into_iter().filter(|event| {
        event.get("request_id").and_then(Value::as_str) == Some(id)
            && event.contains_key("error.type")
    });
    assert_eq!(errors.count(), 1, "the error of {id} is logged once");
    access_line(logs, id)
}

/// An outer layer, added after the contract: it sets the request id by rules of its own, and
/// marks every response it sees.
async fn outer(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(RequestId::new("outer-7"));
    let mut response = next.run(request).await;
    let mark = HeaderValue::from_static("seen");
    response.headers_mut().insert("x-outer", mark);
    response
}

/// Runs every check on the routes with the contract that `keep(routes, expose_internal_detail)`
/// puts on them.
pub(crate) async fn check(keep: impl Fn(Router, bool) -> Router, logs: &LogCapture) {
    let app = keep(routes(), false);

    // Success without data is the envelope; data is bare JSON.
    let ok = get_(&app, "/ok").await;
    assert_eq!(
        (ok.status, ok.body.as_str()),
        (200, r#"{"status":"success","code":200,"description":""}"#)
    );
    assert_eq!(ok.header("content-type"), Some("application/json"));
    let data = get_(&app, "/data").await;
    assert_eq!((data.status, data.body.as_str()), (200, r#"{"id":7}"#));
    let created = send(&app, "POST", "/created", &[], "").await;
    assert_eq!(
        (created.status, created.body.as_str()),
        (201, r#"{"id":8}"#)
    );

    // Each class: its status, what the caller is told, and the access log level.
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
        let reply = get_(&app, &format!("/error/{path}")).await;
        let (outcome, code, said) = reply.envelope();
        assert_eq!((outcome.as_str(), code), ("error", status), "{path}");
        let id = reply.request_id();
        match description.ends_with('=') {
            true => assert_eq!(said, format!("{description}{id})"), "{path}"),
            false => assert_eq!(said, description, "{path}"),
        }
        assert_eq!(reply.header("retry-after"), None, "{path}");
        let line = failed_line(logs, &reply);
        assert_eq!(text(&line, "level"), level, "{path}: {line:?}");
        let kind = if path.contains("titled") {
            "TokenExpired"
        } else {
            "Failing"
        };
        assert_eq!(text(&line, "error.type"), kind, "{line:?}");
        assert!(
            text(&line, "error.context").starts_with("secret detail of"),
            "{line:?}"
        );
    }
    for path in [
        "TooManyRequests?",
        "Unavailable?",
        "TooManyRequests?upstream=true&",
    ] {
        let reply = get_(&app, &format!("/error/{path}retry=true")).await;
        assert_eq!(reply.header("retry-after"), Some("1"), "{path}");
    }
    let reply = get_(&app, "/error/Timeout?retry=true").await;
    assert_eq!(
        reply.header("retry-after"),
        None,
        "only 429 and 503 carry Retry-After"
    );
    let exposed = get_(&keep(routes(), true), "/error/Internal").await;
    let said = exposed.envelope().2;
    assert_eq!(said, "secret detail of Internal", "expose_internal_detail");

    // Errors of the framework.
    let unmatched = get_(&app, "/nope").await;
    assert_eq!(
        unmatched.envelope(),
        ("error".into(), 404, "Not Found".into())
    );
    let line = access_line(logs, unmatched.request_id());
    assert_eq!(
        (text(&line, "http.route"), text(&line, "level")),
        ("", "INFO")
    );
    let method = send(&app, "DELETE", "/ok", &[], "").await;
    assert_eq!(
        method.envelope(),
        ("error".into(), 405, "Method Not Allowed".into())
    );
    assert_eq!(method.header("allow"), Some("GET,HEAD"));
    let json_type = [("content-type", "application/json")];
    let large = format!("{{\"name\":\"{}\"}}", "x".repeat(80));
    let large = send(&app, "POST", "/json", &json_type, &large).await;
    assert_eq!(
        large.envelope(),
        ("error".into(), 413, "Payload Too Large".into())
    );
    let body = r#"{"name":"printer","port":9100}"#;
    let device = send(&app, "POST", "/json", &json_type, body).await;
    assert_eq!((device.status, device.body.as_str()), (200, body));
    let line = Value::Object(access_line(logs, device.request_id()));
    assert!(!line.to_string().contains("printer"), "no bodies in {line}");
    let wrong = r#"{"name":"printer","port":"x"}"#;
    let (_, code, said) = send(&app, "POST", "/json", &json_type, wrong)
        .await
        .envelope();
    assert_eq!(code, 406);
    let expected = "port: invalid type: string \"x\", expected u16 at line 1 column";
    assert!(said.starts_with(expected), "{said}");
    let missing = r#"{"name":"printer"}"#;
    let said = send(&app, "POST", "/json", &json_type, missing)
        .await
        .envelope()
        .2;
    assert!(said.starts_with("missing field `port` at line 1"), "{said}");
    let trailing = r#"{"name":"p","port":1} x"#;
    let said = send(&app, "POST", "/json", &json_type, trailing)
        .await
        .envelope()
        .2;
    assert!(said.starts_with("trailing characters"), "{said}");
    let untyped = send(&app, "POST", "/json", &[], body).await.envelope();
    let expected = "expected a JSON body, with Content-Type: application/json";
    assert_eq!((untyped.1, untyped.2.as_str()), (406, expected));
    let (_, code, said) = get_(&app, "/query?page=two").await.envelope();
    assert_eq!(code, 400);
    assert!(
        said.contains("page: invalid digit found in string"),
        "{said}"
    );
    let (_, code, said) = get_(&app, "/path/seven").await.envelope();
    assert_eq!(code, 400);
    assert!(said.contains("Cannot parse `seven` to a `u32`"), "{said}");
    let slow = get_(&app, "/slow").await;
    let id = slow.request_id();
    let expected = format!("service unavailable (request_id={id})");
    assert_eq!(slow.envelope().2, expected);
    assert_eq!((slow.status, slow.header("retry-after")), (503, Some("1")));
    let line = failed_line(logs, &slow);
    assert_eq!(
        (text(&line, "error.type"), text(&line, "level")),
        ("RequestTimedOut", "WARN")
    );
    let panicked = get_(&app, "/panic").await;
    let id = panicked.request_id();
    let expected = format!("internal error (request_id={id})");
    assert_eq!(panicked.envelope(), ("error".into(), 500, expected));
    let line = failed_line(logs, &panicked);
    assert_eq!(
        (text(&line, "error.type"), text(&line, "level")),
        ("HandlerPanicked", "ERROR")
    );
    let teapot = get_(&app, "/teapot").await;
    assert_eq!(
        teapot.envelope(),
        ("error".into(), 418, "I'm a teapot".into())
    );
    let line = access_line(logs, teapot.request_id());
    assert!(
        text(&line, "level") == "INFO" && !line.contains_key("error.type"),
        "{line:?}"
    );
    let gateway = get_(&app, "/gateway").await;
    let expected = format!("internal error (request_id={})", gateway.request_id());
    assert_eq!(gateway.envelope(), ("error".into(), 502, expected));
    let line = access_line(logs, gateway.request_id());
    assert!(
        text(&line, "level") == "ERROR" && !line.contains_key("error.type"),
        "{line:?}"
    );

    // A response with NoEnvelope keeps its body and headers and carries no error, unless it
    // also has an ApiError's.
    let own = get_(&app, "/own").await;
    assert_eq!(
        (own.status, own.body.as_str()),
        (409, r#"{"failedArr":[3]}"#)
    );
    assert_eq!(own.header("x-own"), Some("kept"));
    let line = access_line(logs, own.request_id());
    assert!(
        text(&line, "level") == "INFO" && !line.contains_key("error.type"),
        "{line:?}"
    );
    let failed = get_(&app, "/own-failed").await;
    assert_eq!(
        failed.envelope(),
        ("error".into(), 409, "device 3 is taken".into())
    );
    assert_eq!(text(&failed_line(logs, &failed), "error.type"), "Conflict");

    // Request ids: the caller's when valid, else a new UUIDv7; in the request's extensions, its
    // span and the response.
    let given = send(&app, "GET", "/id", &[("x-request-id", "abc-123")], "").await;
    assert_eq!(
        (given.request_id(), given.body.as_str()),
        ("abc-123", r#""abc-123""#)
    );
    let longest = "x".repeat(128);
    let kept = send(&app, "GET", "/ok", &[("x-request-id", &longest)], "").await;
    assert_eq!(kept.request_id(), longest);
    for invalid in ["has space", &"x".repeat(129), "", "ü"] {
        let reply = send(&app, "GET", "/id", &[("x-request-id", invalid)], "").await;
        let id = reply.request_id();
        assert!(
            id.len() == 36 && id.as_bytes()[14] == b'7',
            "a UUIDv7, not {id}"
        );
        assert_eq!(reply.body, format!("\"{id}\""));
    }
    let logged = send(&app, "GET", "/log", &[("x-request-id", "log-1")], "").await;
    assert_eq!(logged.status, 200);
    let inside = logs.with_message("inside the handler");
    let spans = inside.last().and_then(|event| event.get("spans")).cloned();
    let request = json!({"name": "request", "request_id": "log-1"});
    let spans = spans
        .and_then(|spans| spans.as_array().cloned())
        .unwrap_or_default();
    assert!(
        spans.contains(&request),
        "the handler logs in the request's span: {spans:?}"
    );

    // An id set by an outer layer wins; outer layers see the final responses.
    let wrapped = keep(routes(), false).layer(middleware::from_fn(outer));
    let reply = send(&wrapped, "GET", "/id", &[("x-request-id", "abc-123")], "").await;
    assert_eq!(
        (reply.request_id(), reply.body.as_str()),
        ("outer-7", r#""outer-7""#)
    );
    for path in ["/slow", "/panic"] {
        let reply = get_(&wrapped, path).await;
        assert_eq!(reply.header("x-outer"), Some("seen"), "{path}");
        assert!(
            reply.envelope().2.contains("(request_id=outer-7)"),
            "{path}"
        );
    }

    // The access log: one line per response, with these fields; DEBUG for the probes.
    let line = access_line(logs, "log-1");
    let expected = ["GET", "/log", "INFO"];
    assert_eq!(
        [&line["http.method"], &line["http.route"], &line["level"]],
        expected
    );
    assert_eq!(line["http.status"], 200);
    assert!(line["latency_ms"].is_u64(), "{line:?}");
    assert!(!line.contains_key("error.type"), "{line:?}");
    let live = get_(&app, "/livez").await;
    assert_eq!(
        text(&access_line(logs, live.request_id()), "level"),
        "DEBUG"
    );
    let ready = get_(&app, "/readyz").await;
    assert_eq!((ready.status, ready.body.as_str()), (503, "not ready"));
    assert_eq!(
        text(&access_line(logs, ready.request_id()), "level"),
        "DEBUG"
    );
}
