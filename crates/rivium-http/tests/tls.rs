#![cfg(feature = "server")]
mod common;
use axum::{Router, routing::get, serve::Listener};
use rivium::{Kind, Outcome, StopReason, Supervisor, scope::Drain};
use rivium_http::{ContractSettings, ServerLimits, contract, serve_listener};
use std::{
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{
    TlsAcceptor, TlsConnector,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
    },
};

struct Handshake(Arc<AtomicUsize>);
impl Drop for Handshake {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct TlsListener {
    tcp: TcpListener,
    tls: TlsAcceptor,
    accepts: Arc<AtomicUsize>,
    handshakes: Arc<AtomicUsize>,
}
impl Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            self.accepts.fetch_add(1, Ordering::SeqCst);
            let (stream, address) = Listener::accept(&mut self.tcp).await;
            self.handshakes.fetch_add(1, Ordering::SeqCst);
            let _handshake = Handshake(self.handshakes.clone());
            // No detached handshake workers: the outer connection permit covers this future.
            if let Ok(stream) = self.tls.accept(stream).await {
                return (stream, address);
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}
fn configurations() -> (TlsAcceptor, TlsConnector) {
    let cert = CertificateDer::from(include_bytes!("fixtures/localhost.der").to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("fixtures/localhost-key.der").to_vec(),
    ));
    let server = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (
        TlsAcceptor::from(Arc::new(server)),
        TlsConnector::from(Arc::new(client)),
    )
}
async fn until(count: &AtomicUsize, value: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while count.load(Ordering::SeqCst) != value {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn tls_handshakes_obey_connection_capacity_and_pending_handshakes_cancel() {
    let (acceptor, connector) = configurations();
    let (tcp, address) = common::bind().await;
    let accepts = Arc::new(AtomicUsize::new(0));
    let handshakes = Arc::new(AtomicUsize::new(0));
    let listener = TlsListener {
        tcp,
        tls: acceptor,
        accepts: accepts.clone(),
        handshakes: handshakes.clone(),
    };
    let router = contract(
        Router::new().route("/", get(|| async { "ok" })),
        &ContractSettings::default(),
        vec![],
    );
    let sup = Supervisor::default().with(rivium::service("tls", Kind::Frontline, move |context| {
        serve_listener(
            listener,
            router,
            context,
            ServerLimits {
                connections: NonZeroUsize::new(1).unwrap(),
            },
        )
    }));
    let (control, _readiness, run) = common::ready(sup).await;
    let tcp = TcpStream::connect(address).await.unwrap();
    let mut first = connector
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    first
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let header = common::headers(&mut first).await;
    assert!(header.starts_with("HTTP/1.1 200"));
    let mut content = [0; 2];
    first.read_exact(&mut content).await.unwrap();
    assert_eq!(&content, b"ok");
    let stalled = TcpStream::connect(address).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 1);
    assert_eq!(handshakes.load(Ordering::SeqCst), 0);
    drop(first);
    until(&handshakes, 1).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 2);
    control.stop(StopReason::Requested);
    let report = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(report.outcome, Outcome::Stopped));
    assert_eq!(report.drain, Drain::Complete);
    assert_eq!(handshakes.load(Ordering::SeqCst), 0);
    drop(stalled);
}
