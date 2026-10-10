//! Stopping an `HttpServer` with connections open: a stream that reads the stop signal, a TLS
//! handshake that never finishes, an idle keep-alive connection and a short request in flight
//! let the stop end in time, and a long request in flight makes it time out. The servers run in
//! an embedded host in this process, started and stopped once per case; each case checks the
//! code that the stop returns, not how long it took, so it holds under emulation as well.

use std::convert::Infallible;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use axum::extract::Path as UrlPath;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use axum::{Extension, Router};
use rivium::embedded::Host;
use rivium::{App, AppContext, Code, Result, Service, StopSignal};
use rivium_http::{Acceptor, ApiResponse, ApiResult, HttpServer, HttpSettings};
use serde::{Deserialize, Serialize};

/// The handlers of `/sleep` that have started.
static SLEEPING: AtomicUsize = AtomicUsize::new(0);
/// The calls of the acceptor.
static ACCEPTED: AtomicUsize = AtomicUsize::new(0);

/// What every stop may take: the services' deadline is then the 2 s of `stop_timeout`.
const BUDGET: Duration = Duration::from_secs(5);

struct Api;

#[derive(Serialize, Deserialize)]
struct Config {
    http: HttpSettings,
    tls: HttpSettings,
}

impl Default for Config {
    fn default() -> Self {
        let mut http = HttpSettings::default();
        (http.addr, http.probes) = (SocketAddr::from(([127, 0, 0, 1], 0)), false);
        let tls = http.clone();
        Config { http, tls }
    }
}

impl App for Api {
    const NAME: &'static str = "stop";
    const VERSION: &'static str = "1.0.0";
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        let http = HttpServer::new(&config.http, routes(), ctx);
        let tls = HttpServer::new(&config.tls, routes(), ctx);
        Ok(vec![
            Box::new(http),
            Box::new(tls.named("tls").accept_with(Peek)),
        ])
    }
}

/// An acceptor whose "handshake" waits for the client's first byte and leaves it for HTTP, as
/// a TLS handshake waits for the client's hello.
struct Peek;

impl Acceptor for Peek {
    type Stream = tokio::net::TcpStream;

    async fn accept(&self, stream: tokio::net::TcpStream) -> io::Result<tokio::net::TcpStream> {
        ACCEPTED.fetch_add(1, Ordering::SeqCst);
        stream.peek(&mut [0; 1]).await?;
        Ok(stream)
    }
}

fn routes() -> Router {
    Router::new()
        .route(
            "/ok",
            get(|| async { ApiResult::<()>::Ok(ApiResponse::Ok) }),
        )
        .route(
            "/sleep/{ms}",
            get(|UrlPath(ms): UrlPath<u64>| async move {
                SLEEPING.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                ApiResult::<()>::Ok(ApiResponse::Ok)
            }),
        )
        .route("/events", get(events))
}

/// Server-sent events, one at once and then one every 100 ms, until the service is asked to
/// stop.
async fn events(Extension(stop): Extension<StopSignal>) -> impl IntoResponse {
    let events = futures_util::stream::unfold((0u64, stop), |(n, stop)| async move {
        if n > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if stop.is_stopping() {
            return None;
        }
        let event = Event::default().data(n.to_string());
        Some((Ok::<_, Infallible>(event), (n + 1, stop)))
    });
    Sse::new(events)
}

/// Starts the host, and returns the addresses of its two servers.
fn start(host: &Host, root: &Path) -> (SocketAddr, SocketAddr) {
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "lifecycle.stop_timeout=2s",
    ];
    let started = host.start(&args.map(String::from));
    assert_eq!(started, Code::Ok, "{:?}", host.last_error());
    (addr_of(root, "http"), addr_of(root, "tls"))
}

/// The address that the server `name` of the latest start listens on.
fn addr_of(root: &Path, name: &str) -> SocketAddr {
    rivium::log::flush(Duration::from_secs(10)).unwrap();
    let log = std::fs::read_to_string(root.join("logs/stop/stop.log")).unwrap();
    let marker = format!("listening service.name={name} listen.addr=");
    let line = log.lines().rev().find(|line| line.contains(&marker));
    let line = line.unwrap_or_else(|| panic!("{log}"));
    let addr = line
        .split(&marker)
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next();
    addr.unwrap().parse().unwrap()
}

/// Waits until `done` holds, for at most 10 s.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A connection with a request sent on it.
fn request(addr: SocketAddr, path: &str, connection: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).unwrap();
    let timeout = Some(Duration::from_secs(30));
    stream.set_read_timeout(timeout).unwrap();
    let head = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: {connection}\r\n\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream
}

/// Reads from `stream` until what it read contains `text`.
fn read_until(stream: &mut TcpStream, text: &str) -> String {
    let mut read = Vec::new();
    let mut buffer = [0; 1024];
    while !String::from_utf8_lossy(&read).contains(text) {
        let n = stream.read(&mut buffer).unwrap();
        assert!(
            n > 0,
            "closed before {text}: {}",
            String::from_utf8_lossy(&read)
        );
        read.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8_lossy(&read).into_owned()
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
fn stopping_with_connections_open() {
    let root = std::env::temp_dir().join(format!("rivium-http-stop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let host = Host::new::<Api>();

    // A stream that reads the stop signal ends by itself.
    let (http, _) = start(&host, &root);
    failures_to_stderr();
    let mut events = request(http, "/events", "keep-alive");
    read_until(&mut events, "data: 0");
    assert_eq!(host.stop(BUDGET), Code::Ok, "{:?}", host.last_error());
    let mut rest = Vec::new();
    events.read_to_end(&mut rest).expect("the stream ended");

    // A TLS handshake that never finishes ends with the listener.
    let (_, tls) = start(&host, &root);
    let accepted = ACCEPTED.load(Ordering::SeqCst);
    let stalled = TcpStream::connect(tls).unwrap();
    wait_for("the acceptor's call", || {
        ACCEPTED.load(Ordering::SeqCst) > accepted
    });
    assert_eq!(host.stop(BUDGET), Code::Ok, "{:?}", host.last_error());
    drop(stalled);

    // An idle keep-alive connection is closed.
    let (http, _) = start(&host, &root);
    let mut idle = request(http, "/ok", "keep-alive");
    read_until(&mut idle, r#""description":""}"#);
    assert_eq!(host.stop(BUDGET), Code::Ok, "{:?}", host.last_error());
    assert_eq!(idle.read(&mut [0; 16]).unwrap(), 0, "closed");

    // A short request in flight is answered before the stop ends.
    let (http, _) = start(&host, &root);
    let sleeping = SLEEPING.load(Ordering::SeqCst);
    let client = thread::spawn(move || {
        let mut stream = request(http, "/sleep/300", "close");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).map(|_| reply)
    });
    wait_for("the handler's start", || {
        SLEEPING.load(Ordering::SeqCst) > sleeping
    });
    assert_eq!(host.stop(BUDGET), Code::Ok, "{:?}", host.last_error());
    let reply = client.join().unwrap().unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");

    // A long request still running at the deadline makes the stop time out.
    let (http, _) = start(&host, &root);
    let sleeping = SLEEPING.load(Ordering::SeqCst);
    let client = thread::spawn(move || {
        let mut stream = request(http, "/sleep/20000", "close");
        let mut reply = Vec::new();
        let _ = stream.read_to_end(&mut reply);
        reply
    });
    wait_for("the handler's start", || {
        SLEEPING.load(Ordering::SeqCst) > sleeping
    });
    assert_eq!(host.stop(BUDGET), Code::StopTimedOut);
    assert!(client.join().unwrap().is_empty(), "no answer");

    let _ = std::fs::remove_dir_all(&root);
}
