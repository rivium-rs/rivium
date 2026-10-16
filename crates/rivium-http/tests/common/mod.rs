// Independent test binaries share transport and startup setup without hiding assertions.
#![allow(dead_code)]
use rivium::{Control, Readiness, RunReport, Supervisor};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
pub(crate) async fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    (listener, address)
}
pub(crate) async fn ready(supervisor: Supervisor) -> (Control, Readiness, JoinHandle<RunReport>) {
    let control = supervisor.control();
    let mut readiness = supervisor.readiness();
    let run = tokio::spawn(supervisor.run());
    rivium_test::until_ready(&mut readiness).await.unwrap();
    (control, readiness, run)
}
pub(crate) async fn send(stream: &mut TcpStream, path: &str) {
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
}
pub(crate) async fn headers(stream: &mut (impl AsyncRead + Unpin)) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            bytes.push(stream.read_u8().await.unwrap());
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .unwrap()
}
pub(crate) async fn call(address: SocketAddr, path: &str, id: Option<&str>) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let id = id
        .map(|id| format!("x-request-id: {id}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{id}Connection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    reply
}
pub(crate) async fn response(
    router: &axum::Router,
    request: axum::extract::Request,
) -> axum::response::Response {
    use tower::ServiceExt;
    router.clone().oneshot(request).await.unwrap()
}
pub(crate) fn request(path: &str) -> axum::extract::Request {
    axum::http::Request::get(path)
        .body(axum::body::Body::empty())
        .unwrap()
}
pub(crate) async fn body_text(response: impl axum::response::IntoResponse, limit: usize) -> String {
    let bytes = axum::body::to_bytes(response.into_response().into_body(), limit)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}
