#![cfg(feature = "server")]

mod common;

use axum::{Router, routing::get, serve::Listener};
use common::{headers, send};
use rivium::{Kind, Outcome, Service, StopReason, Supervisor, service};
use rivium_http::{ContractSettings, HttpServer, ServerLimits, serve_listener};
use std::{
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Notify,
};

struct Counted {
    inner: TcpListener,
    accepts: Arc<AtomicUsize>,
}

impl Listener for Counted {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        self.accepts.fetch_add(1, Ordering::SeqCst);
        Listener::accept(&mut self.inner).await
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

fn custom(listener: TcpListener, router: Router, accepts: Arc<AtomicUsize>) -> Box<dyn Service> {
    service("custom", Kind::Frontline, move |context| {
        serve_listener(
            Counted {
                inner: listener,
                accepts,
            },
            router,
            context,
            ServerLimits {
                connections: NonZeroUsize::new(1).unwrap(),
            },
        )
    })
}

async fn wait_for(count: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while count.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn binding_and_self_ready_do_not_accept_until_every_dependency_is_ready() {
    let (listener, address) = common::bind().await;
    let accepts = Arc::new(AtomicUsize::new(0));
    let (ready, gate) = tokio::sync::oneshot::channel();
    let sup = Supervisor::default()
        .with(custom(
            listener,
            Router::new().route("/", get(|| async { "ready" })),
            accepts.clone(),
        ))
        .with(service("dependency", Kind::Background, |cx| async move {
            gate.await.unwrap();
            cx.ready();
            cx.stopped().await;
            Ok(())
        }));
    let control = sup.control();
    let mut readiness = sup.readiness();
    let run = tokio::spawn(sup.run());
    let mut client = TcpStream::connect(address).await.unwrap();
    send(&mut client, "/").await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 0);
    ready.send(()).unwrap();
    rivium_test::until_ready(&mut readiness).await.unwrap();
    assert!(headers(&mut client).await.starts_with("HTTP/1.1 200"));
    control.stop(StopReason::Requested);
    drop(client);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
}

#[tokio::test]
async fn connection_budget_is_acquired_before_polling_the_custom_acceptor() {
    let (listener, address) = common::bind().await;
    let accepts = Arc::new(AtomicUsize::new(0));
    let sup = Supervisor::default().with(custom(listener, Router::new(), accepts.clone()));
    let (control, _readiness, run) = common::ready(sup).await;
    let first = TcpStream::connect(address).await.unwrap();
    wait_for(&accepts, 1).await;
    let second = TcpStream::connect(address).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 1);
    drop(first);
    wait_for(&accepts, 2).await;
    control.stop(StopReason::Requested);
    drop(second);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
}

#[tokio::test]
async fn five_hundred_connection_budget_still_limits_inflight_requests_to_256() {
    let (listener, address) = common::bind().await;
    let count = Arc::new(AtomicUsize::new(0));
    let entered = count.clone();
    let released = Arc::new(AtomicBool::new(false));
    let gate = released.clone();
    let notify = Arc::new(Notify::new());
    let wake = notify.clone();
    let router = Router::new().route(
        "/wait",
        get(move || {
            let entered = entered.clone();
            let gate = gate.clone();
            let wake = wake.clone();
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                loop {
                    let notified = wake.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if gate.load(Ordering::Acquire) {
                        break;
                    }
                    notified.await;
                }
                "done"
            }
        }),
    );
    let server = HttpServer::from_listener("http", listener, router)
        .unwrap()
        .limits(ServerLimits {
            connections: NonZeroUsize::new(512).unwrap(),
        })
        .settings(ContractSettings {
            request_timeout_ms: 10_000,
            ..Default::default()
        });
    let sup = Supervisor::default().with(Box::new(server));
    let (control, _readiness, run) = common::ready(sup).await;
    let mut connections = Vec::new();
    for _ in 0..256 {
        let mut client = TcpStream::connect(address).await.unwrap();
        send(&mut client, "/wait").await;
        connections.push(client);
    }
    wait_for(&count, 256).await;
    let mut excess = TcpStream::connect(address).await.unwrap();
    send(&mut excess, "/wait").await;
    let overloaded = headers(&mut excess).await;
    // Release all real handlers before assertions to avoid leaking the fixture on failure.
    released.store(true, Ordering::Release);
    notify.notify_waiters();
    assert!(overloaded.starts_with("HTTP/1.1 503"), "{overloaded}");
    assert_eq!(count.load(Ordering::SeqCst), 256);
    drop(connections);
    drop(excess);
    control.stop(StopReason::Requested);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
}
