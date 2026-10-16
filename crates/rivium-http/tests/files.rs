#![cfg(feature = "fs")]

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use rivium_test::TempDir;

#[tokio::test]
async fn downloads_preserve_ranges_and_head_responses() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("log.txt");
    std::fs::write(&path, b"abcdef").unwrap();
    let request = Request::builder()
        .header(header::RANGE, "bytes=1-3")
        .body(Body::empty())
        .unwrap();
    let response = rivium_http::file_response(request, &path, "日志.txt").await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("filename*=UTF-8''")
    );
    assert_eq!(to_bytes(response.into_body(), 16).await.unwrap(), "bcd");

    let request = Request::builder()
        .method("HEAD")
        .body(Body::empty())
        .unwrap();
    let response = rivium_http::file_response(request, &path, "log.txt").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
    assert!(to_bytes(response.into_body(), 16).await.unwrap().is_empty());
}
