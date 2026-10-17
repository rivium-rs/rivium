//! Open-loop loopback probe. Identical source can run against the historical HTTP API.
//! Application body ownership is a proxy, not the server's internal permit counter.
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::Request,
    routing::{get, post},
};
use rivium::{Outcome, Supervisor, rt::StopReason};
use rivium_http::HttpServer;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
};

#[derive(Default)]
struct Counts {
    active: AtomicUsize,
    high: AtomicUsize,
}

struct Owner(Arc<Counts>);

impl Owner {
    fn new(counts: Arc<Counts>) -> Self {
        let active = counts.active.fetch_add(1, Ordering::SeqCst) + 1;
        counts.high.fetch_max(active, Ordering::SeqCst);
        Self(counts)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn handler(request: Request, counts: Arc<Counts>) -> Body {
    let owner = Owner::new(counts);
    let slow = request.uri().path() == "/slow";
    if request.method() == "POST" {
        let body = to_bytes(request.into_body(), 65536).await.unwrap();
        assert_eq!(body.len(), 65536);
        assert!(body.iter().all(|byte| *byte == b'p'));
    }
    Body::from_stream(futures_util::stream::once(async move {
        if slow {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // The guard remains in the stream until its payload is produced or dropped.
        let _owner = owner;
        Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 1024]))
    }))
}

async fn request(address: SocketAddr, post: bool, mode: usize) -> std::io::Result<u16> {
    let mut stream = TcpStream::connect(address).await?;
    if mode == 2 {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let path = if mode == 1 { "/slow" } else { "/" };
    let method = if post { "POST" } else { "GET" };
    let length = if post { 65536 } else { 0 };
    stream
        .write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {length}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    if post {
        stream.write_all(&vec![b'p'; 65536]).await?;
    }
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    let status = std::str::from_utf8(bytes.get(9..12).unwrap_or_default())
        .ok()
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    if status == 200 {
        assert!(
            complete_payload(&bytes),
            "incomplete or malformed HTTP response"
        );
    }
    Ok(status)
}

// Parse only the fixture's HTTP/1 framing; trailers and chunk extensions are not emitted.
fn complete_payload(response: &[u8]) -> bool {
    let Some(boundary) = response.windows(4).position(|part| part == b"\r\n\r\n") else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&response[..boundary]) else {
        return false;
    };
    let body = &response[boundary + 4..];
    let headers = headers.to_ascii_lowercase();
    if headers
        .lines()
        .any(|line| line == "transfer-encoding: chunked")
    {
        let mut remaining = body;
        let mut decoded = Vec::new();
        loop {
            let Some(line) = remaining.windows(2).position(|part| part == b"\r\n") else {
                return false;
            };
            let Some(size) = std::str::from_utf8(&remaining[..line])
                .ok()
                .and_then(|line| usize::from_str_radix(line, 16).ok())
            else {
                return false;
            };
            remaining = &remaining[line + 2..];
            if size == 0 {
                return remaining == b"\r\n" && decoded == vec![b'x'; 1024];
            }
            if size > remaining.len().saturating_sub(2) || &remaining[size..size + 2] != b"\r\n" {
                return false;
            }
            decoded.extend_from_slice(&remaining[..size]);
            if decoded.len() > 1024 {
                return false;
            }
            remaining = &remaining[size + 2..];
        }
    }
    let length = headers.lines().find_map(|line| {
        line.strip_prefix("content-length: ")
            .and_then(|length| length.parse::<usize>().ok())
    });
    length == Some(1024) && body == vec![b'x'; 1024]
}

fn framing_self_check() {
    let mut fixed = b"HTTP/1.1 200 OK\r\ncontent-length: 1024\r\n\r\n".to_vec();
    fixed.extend_from_slice(&[b'x'; 1024]);
    assert!(complete_payload(&fixed));
    assert!(!complete_payload(&fixed[..fixed.len() - 1]));
    let mut chunked = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n400\r\n".to_vec();
    chunked.extend_from_slice(&[b'x'; 1024]);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    assert!(complete_payload(&chunked));
    assert!(!complete_payload(&chunked[..chunked.len() - 1]));
    assert!(!complete_payload(&chunked[..chunked.len() - 7]));
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

async fn load(
    address: SocketAddr,
    workers: usize,
    capacity: usize,
    post: bool,
    mixed: bool,
    seconds: f64,
) {
    let rate = (capacity * 20).min(4000);
    let offered = (seconds * rate as f64) as usize;
    let slots = Arc::new(Semaphore::new(capacity));
    let start = Instant::now();
    let mut tasks = JoinSet::new();
    let mut suppressed = 0;
    let mut lag = Vec::new();
    let mut raw = Vec::new();
    for index in 0..offered {
        let planned = start + Duration::from_secs_f64(index as f64 / rate as f64);
        tokio::time::sleep_until(planned.into()).await;
        lag.push(planned.elapsed().as_micros());
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            // Count generator-capacity omissions separately; never hide them as successes.
            suppressed += 1;
            raw.push((
                index,
                (planned - start).as_micros(),
                start.elapsed().as_micros(),
                0,
                0,
                "client_suppressed",
            ));
            continue;
        };
        let mode = if mixed && index % 10 < 2 {
            index % 10 + 1
        } else {
            0
        };
        tasks.spawn(async move {
            let _permit = permit;
            let sent = start.elapsed().as_micros();
            let result =
                tokio::time::timeout(Duration::from_secs(5), request(address, post, mode)).await;
            let status = match result {
                Ok(Ok(status)) => status,
                _ => 0,
            };
            (
                index,
                (planned - start).as_micros(),
                sent,
                status,
                planned.elapsed().as_micros(),
            )
        });
    }
    let mut success = Vec::new();
    let mut all = Vec::new();
    let mut rejected = 0;
    let mut errors = 0;
    while let Some(result) = tasks.join_next().await {
        let (index, planned, sent, status, elapsed) = result.unwrap();
        raw.push((index, planned, sent, status, elapsed, "attempt"));
        all.push(elapsed);
        match status {
            200 => success.push(elapsed),
            503 => rejected += 1,
            _ => errors += 1,
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(success.len() + rejected + errors + suppressed, offered);
    if let Ok(directory) = std::env::var("RIVIUM_HTTP_RAW") {
        use std::io::Write;
        std::fs::create_dir_all(&directory).unwrap();
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(format!(
                "{directory}/{workers}-{capacity}-{post}-{mixed}.csv"
            ))
            .unwrap(),
        );
        writeln!(
            file,
            "index,planned_us,attempt_started_us,status,planned_to_complete_us,outcome"
        )
        .unwrap();
        raw.sort_unstable_by_key(|row| row.0);
        for (index, planned, sent, status, elapsed, outcome) in raw {
            writeln!(
                file,
                "{index},{planned},{sent},{status},{elapsed},{outcome}"
            )
            .unwrap();
        }
    }
    success.sort_unstable();
    all.sort_unstable();
    lag.sort_unstable();
    // Recovery is a separate real request after all offered work has completed.
    let recovery = Instant::now();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), request(address, false, 0))
            .await
            .unwrap()
            .unwrap(),
        200
    );
    println!(
        "{capacity},{},{},{rate},{offered},{},{rejected},{errors},{suppressed},{:.3},{:.3},{},{},{},{},{},{},{},{}",
        if post { "post_64k" } else { "get_1k" },
        if mixed {
            "slow_body_idle_20pct"
        } else {
            "normal"
        },
        success.len(),
        elapsed,
        success.len() as f64 / elapsed,
        percentile(&success, 50),
        percentile(&success, 95),
        percentile(&success, 99),
        percentile(&all, 50),
        percentile(&all, 95),
        percentile(&all, 99),
        percentile(&lag, 99),
        recovery.elapsed().as_micros()
    );
}

fn main() {
    framing_self_check();
    let smoke = std::env::args().any(|arg| arg == "--test");
    let seconds = if smoke { 0.05 } else { 3.0 };
    println!(
        "workers,capacity,payload,mode,offered_per_s,offered,success,http_503,errors,client_suppressed,elapsed_s,goodput_per_s,success_p50_us,success_p95_us,success_p99_us,all_attempt_p50_us,all_attempt_p95_us,all_attempt_p99_us,schedule_lag_p99_us,recovery_us"
    );
    for workers in [1, 4] {
        let server = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .enable_all()
            .build()
            .unwrap();
        let counts = Arc::new(Counts::default());
        let (address, control, run) = server.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let normal = counts.clone();
            let slow = counts.clone();
            let router = Router::new()
                .route(
                    "/",
                    get({
                        let normal = normal.clone();
                        move |request| handler(request, normal.clone())
                    })
                    .post(move |request| handler(request, normal.clone())),
                )
                .route(
                    "/slow",
                    post({
                        let slow = slow.clone();
                        move |request| handler(request, slow.clone())
                    })
                    .get(move |request| handler(request, slow.clone())),
                );
            let supervisor = Supervisor::default().with(Box::new(
                HttpServer::from_listener("http", listener, router).unwrap(),
            ));
            let control = supervisor.control();
            let mut readiness = supervisor.readiness();
            let run = tokio::spawn(supervisor.run());
            rivium_test::until_ready(&mut readiness).await.unwrap();
            (address, control, run)
        });
        let client = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        client.block_on(async {
            assert_eq!(request(address, false, 0).await.unwrap(), 200);
            for capacity in [1, 32, 128, 256, 512, 1024] {
                for post in [false, true] {
                    for mixed in [false, true] {
                        for _ in 0..32 {
                            assert_eq!(request(address, post, 0).await.unwrap(), 200);
                        }
                        counts.high.store(0, Ordering::SeqCst);
                        let case = format!("{workers}-{capacity}-{post}-{mixed}");
                        eprintln!(
                            "phase=case_start case={case} unix_ms={} pid={}",
                            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            std::process::id()
                        );
                        print!("{workers},");
                        load(address, workers, capacity, post, mixed, seconds).await;
                        tokio::time::timeout(Duration::from_secs(5), async {
                            while counts.active.load(Ordering::SeqCst) != 0 {
                                tokio::task::yield_now().await;
                            }
                        })
                        .await
                        .unwrap();
                        eprintln!(
                            "phase=case_end case={case} unix_ms={} pid={} workers={workers} capacity={capacity} post={post} mixed={mixed} application_body_high={} application_body_after=0",
                            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            std::process::id(),
                            counts.high.load(Ordering::SeqCst)
                        );
                    }
                }
            }
        });
        control.stop(StopReason::Requested);
        server.block_on(async {
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(10), run)
                    .await
                    .unwrap()
                    .unwrap()
                    .outcome,
                Outcome::Stopped
            ));
        });
    }
}
