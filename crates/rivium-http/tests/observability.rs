#![cfg(feature = "server")]

mod common;

use axum::{Router, routing::get};
use rivium::{Error, StopReason, Supervisor, error::kinds};
use rivium_http::{ApiError, HttpServer};
use tracing::instrument::WithSubscriber;

#[tokio::test]
async fn detached_requests_inherit_the_scoped_service_dispatch_and_request_span() {
    let logs = rivium_test::Logs::default();
    let (listener, address) = common::bind().await;
    let router = Router::new().route(
        "/error",
        get(|| async {
            tracing::info!("inside request");
            ApiError(Error::internal("diagnostic"))
        }),
    );
    let router = router
        .route(
            "/unavailable",
            get(|| async { ApiError(Error::new(kinds::UNAVAILABLE, "dependency unavailable")) }),
        )
        .route(
            "/busy",
            get(|| async { ApiError(Error::new(kinds::TOO_MANY_REQUESTS, "quota")) }),
        )
        .route("/ok", get(|| async { "ok" }));
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish(),
    );
    let supervisor = Supervisor::default().with(Box::new(
        HttpServer::from_listener("observed", listener, router).unwrap(),
    ));
    let control = supervisor.control();
    let mut readiness = supervisor.readiness();
    let run = tokio::spawn(supervisor.run().with_subscriber(dispatch));
    rivium_test::until_ready(&mut readiness).await.unwrap();
    let response = common::call(address, "/error", Some("observable")).await;
    assert!(response.starts_with("HTTP/1.1 500"));
    for path in ["/unavailable", "/busy", "/ok", "/livez", "/readyz"] {
        common::call(address, path, None).await;
    }
    control.stop(StopReason::Requested);
    assert!(run.await.unwrap().drain.is_complete());
    let text = logs.text();
    let handler = text
        .lines()
        .find(|line| line.contains("inside request"))
        .unwrap_or_else(|| panic!("handler lost scoped subscriber: {text}"));
    assert!(handler.contains("service=observed"), "{handler}");
    assert!(handler.contains("request_id=observable"), "{handler}");
    let access: Vec<_> = text
        .lines()
        .filter(|line| line.contains("status=500"))
        .collect();
    assert_eq!(access.len(), 1, "{text}");
    assert!(access[0].contains("ERROR"), "{}", access[0]);
    for (path, level) in [
        ("/unavailable", "WARN"),
        ("/busy", "WARN"),
        ("/ok", "INFO"),
        ("/livez", "DEBUG"),
        ("/readyz", "DEBUG"),
    ] {
        let events: Vec<_> = text
            .lines()
            .filter(|line| line.contains(&format!("path={path} ")))
            .collect();
        assert_eq!(events.len(), 1, "{path}: {text}");
        assert!(events[0].contains(level), "{}", events[0]);
    }
}
