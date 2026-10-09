//! Platform smoke checks shared by the tests and by `smoke check`: an axum round trip and a TLS
//! handshake with rustls and the ring provider, the TLS stack that services built on Rivium use
//! (Rivium itself has no TLS). The tests run on every target, under emulation for the
//! cross-compiled ones, and the glibc checks read the binary.

use std::error::Error;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::get};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, Connection, RootCertStore, ServerConfig, ServerConnection,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Result of a smoke check.
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const CA: &[u8] = include_bytes!("../data/ca.der");
const LEAF: &[u8] = include_bytes!("../data/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("../data/leaf.key.der");

/// Serves `GET /hello` with axum on a loopback port, requests it over TCP, shuts the server down
/// gracefully and returns the raw HTTP response.
pub async fn http_round_trip() -> Result<String> {
    let app = Router::new().route("/hello", get(|| async { "hello" }));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });

    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), server).await???;
    Ok(response)
}

/// Runs a TLS 1.3 handshake between an in-memory rustls client and server using the ring
/// provider, verifying a test certificate chain (valid until 2126), then sends `ping` from the
/// client to the server. Returns the negotiated protocol version and cipher suite.
pub fn tls_round_trip() -> Result<String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(CA))?;
    let client_config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(LEAF)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(LEAF_KEY)),
        )?;
    let name = ServerName::try_from("localhost")?;
    let mut client = Connection::Client(ClientConnection::new(Arc::new(client_config), name)?);
    let mut server = Connection::Server(ServerConnection::new(Arc::new(server_config))?);

    while client.is_handshaking() || server.is_handshaking() {
        transfer(&mut client, &mut server)?;
        transfer(&mut server, &mut client)?;
    }
    client.writer().write_all(b"ping")?;
    transfer(&mut client, &mut server)?;
    let mut received = [0u8; 4];
    server.reader().read_exact(&mut received)?;
    if &received != b"ping" {
        return Err("application data was corrupted".into());
    }
    let suite = client
        .negotiated_cipher_suite()
        .ok_or("no cipher suite negotiated")?;
    Ok(format!(
        "{:?} {:?}",
        client.protocol_version(),
        suite.suite()
    ))
}

fn transfer(from: &mut Connection, to: &mut Connection) -> Result<()> {
    let mut records = Vec::new();
    while from.wants_write() {
        from.write_tls(&mut records)?;
    }
    let mut records = &records[..];
    while !records.is_empty() {
        to.read_tls(&mut records)?;
        to.process_new_packets()?;
    }
    Ok(())
}
