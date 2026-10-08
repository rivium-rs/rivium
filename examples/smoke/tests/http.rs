//! axum: serves a request over loopback TCP and shuts down gracefully.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_a_request_and_shuts_down() {
    let response = smoke::http_round_trip().await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("hello"), "{response}");
}
