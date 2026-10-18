#![cfg(feature = "server")]

mod common;

use axum::{Router, body::Body, routing::get, serve::Listener};
use rivium::{Kind, Service, scope::Drain};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

#[derive(Clone, Copy)]
enum Entry {
    Server,
    CustomListener,
}

// A custom listener can return a distinct IO type, as TLS listeners do.
struct CustomListener(TcpListener);

impl Listener for CustomListener {
    type Io = Box<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, addr) = Listener::accept(&mut self.0).await;
        (Box::new(io), addr)
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

fn server(
    listener: TcpListener,
    router: Router,
    scope: Arc<OnceLock<rivium::scope::TaskScope>>,
    entry: Entry,
) -> Box<dyn Service> {
    rivium::service("http", Kind::Frontline, move |context| async move {
        assert!(scope.set(context.task_scope()).is_ok());
        match entry {
            Entry::Server => {
                Box::new(rivium_http::HttpServer::from_listener("http", listener, router).unwrap())
                    .run(context)
                    .await
            }
            Entry::CustomListener => {
                rivium_http::serve_listener(
                    CustomListener(listener),
                    router,
                    context,
                    rivium_http::ServerLimits::default(),
                )
                .await
            }
        }
    })
}

struct Resource {
    active: Arc<AtomicUsize>,
    untracked_drops: Arc<AtomicUsize>,
    scope: Arc<OnceLock<rivium::scope::TaskScope>>,
}

impl Drop for Resource {
    fn drop(&mut self) {
        if self.scope.get().unwrap().active_tasks() == 0 {
            self.untracked_drops.fetch_add(1, Ordering::SeqCst);
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn forced_stop_keeps_request_owned(entry: Entry, streaming: bool) {
    let active = Arc::new(AtomicUsize::new(0));
    let untracked_drops = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let scope = Arc::new(OnceLock::new());
    let (a, d, e, r, s) = (
        active.clone(),
        untracked_drops.clone(),
        entered.clone(),
        release.clone(),
        scope.clone(),
    );
    let router = Router::new().route(
        "/wait",
        get(move || {
            let (active, untracked_drops, entered, release, scope) =
                (a.clone(), d.clone(), e.clone(), r.clone(), s.clone());
            async move {
                active.fetch_add(1, Ordering::SeqCst);
                let resource = Resource {
                    active,
                    untracked_drops,
                    scope,
                };
                if streaming {
                    Body::from_stream(futures_util::stream::once(async move {
                        let _resource = resource;
                        entered.notify_one();
                        release.notified().await;
                        Ok::<_, Infallible>("drained")
                    }))
                } else {
                    let _resource = resource;
                    entered.notify_one();
                    release.notified().await;
                    Body::from("drained")
                }
            }
        }),
    );
    let (listener, addr) = common::bind().await;
    let supervisor = rivium::Supervisor::new(Duration::from_secs(1), Duration::from_millis(20))
        .with(server(listener, router, scope, entry));
    let (control, readiness, run) = common::ready(supervisor).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    common::send(&mut client, "/wait").await;
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    control.stop(rivium::rt::StopReason::Requested);
    let outcome = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome.outcome, rivium::Outcome::TimedOut { .. }));
    assert_eq!(active.load(Ordering::SeqCst), 1);
    let before_release = readiness.drain(Duration::from_millis(10)).await;

    // Always release the handler/body, including when the ownership assertion fails.
    release.notify_one();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(1), client.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.contains("drained"));
    assert_eq!(
        readiness.drain(Duration::from_secs(1)).await,
        Drain::Complete
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(matches!(before_release, Drain::Incomplete { .. }));
    assert_eq!(untracked_drops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn server_handler_stays_owned_after_root_abort() {
    forced_stop_keeps_request_owned(Entry::Server, false).await;
}

#[tokio::test]
async fn server_streaming_body_stays_owned_after_root_abort() {
    forced_stop_keeps_request_owned(Entry::Server, true).await;
}

#[tokio::test]
async fn custom_listener_handler_stays_owned_after_root_abort() {
    forced_stop_keeps_request_owned(Entry::CustomListener, false).await;
}

#[tokio::test]
async fn custom_listener_streaming_body_stays_owned_after_root_abort() {
    forced_stop_keeps_request_owned(Entry::CustomListener, true).await;
}

async fn listener_admission(entry: Entry, close_first: bool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let router = Router::new().route(
        "/",
        get(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { "accepted" }
        }),
    );
    let (listener, addr) = common::bind().await;
    let scope = Arc::new(OnceLock::new());
    let supervisor = rivium::Supervisor::new(Duration::from_secs(1), Duration::from_secs(1))
        .with(server(listener, router, scope.clone(), entry));
    let (control, readiness, run) = common::ready(supervisor).await;
    let scope = scope.get().unwrap();
    if close_first {
        scope.close();
    }
    let mut client = TcpStream::connect(addr).await.unwrap();
    if close_first {
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut reply = [0; 64];
        let received = tokio::time::timeout(Duration::from_secs(1), client.read(&mut reply))
            .await
            .unwrap();
        assert!(received.is_err() || received.unwrap() == 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(scope.active_tasks(), 0);
    } else {
        // A TCP connection is owned even before it sends its first HTTP request.
        tokio::time::timeout(Duration::from_secs(1), async {
            while scope.active_tasks() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    control.stop(rivium::rt::StopReason::Requested);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .unwrap()
            .unwrap()
            .outcome,
        rivium::Outcome::Stopped
    ));
    assert_eq!(
        readiness.drain(Duration::from_secs(1)).await,
        Drain::Complete
    );
}

#[tokio::test]
async fn idle_connections_are_owned_until_shutdown() {
    for entry in [Entry::Server, Entry::CustomListener] {
        listener_admission(entry, false).await;
    }
}

#[tokio::test]
async fn closed_scope_rejects_new_connections() {
    for entry in [Entry::Server, Entry::CustomListener] {
        listener_admission(entry, true).await;
    }
}
