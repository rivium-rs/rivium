#![cfg(feature = "server")]
mod common;

use axum::{Router, routing::get};

#[tokio::test]
async fn real_server_probes_and_inflight_shutdown() {
    let (listener, addr) = common::bind().await;
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let router = Router::new().route(
        "/wait",
        get(
            move |axum::Extension(stop): axum::Extension<rivium::Stop>| {
                let notify = notify.clone();
                async move {
                    notify.notify_one();
                    stop.stopped().await;
                    "drained"
                }
            },
        ),
    );
    let router =
        router.route(
            "/peer",
            get(
                |axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<
                    std::net::SocketAddr,
                >| async move { peer.ip().to_string() },
            ),
        );
    let server = rivium_http::HttpServer::from_listener("http", listener, router).unwrap();
    let sup = rivium::Supervisor::default().with(Box::new(server));
    let control = sup.control();
    let mut readiness = sup.readiness();
    let check = readiness.health().register("database").unwrap();
    check.set(rivium::Health::Healthy);
    let run = tokio::spawn(sup.run());
    rivium_test::until_ready(&mut readiness).await.unwrap();
    assert!(
        common::call(addr, "/readyz", None)
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        common::call(addr, "/peer", None)
            .await
            .ends_with("127.0.0.1")
    );
    check.set(rivium::Health::Unhealthy("database unavailable".into()));
    assert!(
        common::call(addr, "/readyz", None)
            .await
            .starts_with("HTTP/1.1 503")
    );
    check.set(rivium::Health::Healthy);
    let request = tokio::spawn(common::call(addr, "/wait", None));
    entered.notified().await;
    control.stop(rivium::rt::StopReason::Requested);
    let response = request.await.unwrap();
    assert!(response.contains("drained"));
    assert!(matches!(
        run.await.unwrap().outcome,
        rivium::Outcome::Stopped
    ));
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}
