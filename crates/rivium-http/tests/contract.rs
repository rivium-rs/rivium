#![cfg(feature = "contract")]
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use rivium_http::{ApiError, ApiResponse, ContractSettings, Json, NoEnvelope, contract};

fn json_app(body_limit: usize) -> Router {
    contract(
        Router::new().route(
            "/",
            post(|Json(_): Json<serde_json::Value>| async { StatusCode::OK }),
        ),
        &ContractSettings {
            body_limit,
            ..Default::default()
        },
        vec![],
    )
}
async fn request(
    router: Router,
    path: &str,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let response = common::response(
        &router,
        Request::builder()
            .uri(path)
            .header("x-request-id", "test-id")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn contract_wraps_framework_errors_and_hides_internal_details() {
    let router = contract(
        Router::new().route(
            "/secret",
            get(|| async { ApiError(rivium::Error::internal("password=secret")) }),
        ),
        &ContractSettings::default(),
        vec![],
    );
    for path in ["/missing", "/secret"] {
        let (status, headers, body) = request(router.clone(), path).await;
        assert_eq!(headers["x-request-id"], "test-id");
        assert!(status.is_client_error() || status.is_server_error());
        assert_eq!(body["code"], status.as_u16());
        assert!(!body.to_string().contains("password"));
    }
}

#[tokio::test]
async fn panic_and_timeout_are_enveloped() {
    let settings = ContractSettings {
        request_timeout_ms: 10,
        ..Default::default()
    };
    let router = contract(
        Router::new()
            .route(
                "/panic",
                get(|| async {
                    panic!("secret");

                    #[allow(unreachable_code)]
                    StatusCode::OK
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    StatusCode::OK
                }),
            ),
        &settings,
        vec![],
    );
    assert_eq!(
        request(router.clone(), "/panic").await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        request(router, "/slow").await.0,
        StatusCode::GATEWAY_TIMEOUT
    );
}

#[tokio::test]
async fn raw_escape_hatch_and_success_data() {
    let router = contract(
        Router::new()
            .route(
                "/raw",
                get(|| async {
                    let mut response = (StatusCode::BAD_REQUEST, "raw").into_response();
                    response.extensions_mut().insert(NoEnvelope);
                    response
                }),
            )
            .route(
                "/data",
                get(|| async { ApiResponse::Data(serde_json::json!({"answer":42})) }),
            ),
        &ContractSettings::default(),
        vec![],
    );
    let response = common::response(&router, common::request("/raw")).await;
    assert_eq!(to_bytes(response.into_body(), 1000).await.unwrap(), "raw");
    assert_eq!(request(router, "/data").await.2["answer"], 42);
}

#[tokio::test]
async fn malformed_json_and_body_limit() {
    let router = json_app(10);
    for (body, status) in [("invalid", 406), ("abcdefghijklmnop", 413)] {
        let response = common::response(
            &router,
            Request::post("/")
                .header("content-type", "application/json")
                .header("content-length", body.len())
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status().as_u16(), status);
    }
}

#[tokio::test]
async fn chunked_body_limits_apply() {
    let router = json_app(4);
    let body = Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(
        "{\"long\":\"value\"}",
    )]));
    let response = common::response(
        &router,
        Request::post("/")
            .header("content-type", "application/json")
            .body(body)
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn oversized_content_length_is_rejected_even_when_the_route_ignores_the_body() {
    let router = contract(
        Router::new().route("/", post(|| async { StatusCode::OK })),
        &ContractSettings::default(),
        vec![],
    );
    let response = common::response(
        &router,
        Request::post("/")
            .header("content-length", u64::from(u32::MAX) + 1)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn conflicting_content_lengths_are_rejected_before_the_handler() {
    let app = contract(
        Router::new().route("/", post(|| async { StatusCode::OK })),
        &ContractSettings {
            body_limit: 10,
            ..Default::default()
        },
        vec![],
    );
    let response = common::response(
        &app,
        Request::post("/")
            .header("content-length", "1")
            .header("content-length", "100")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn replacing_an_error_body_does_not_retain_its_encoding_header() {
    let app = contract(
        Router::new().route(
            "/",
            get(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    [("content-encoding", "gzip")],
                    "obsolete encoded body",
                )
            }),
        ),
        &ContractSettings::default(),
        vec![],
    );
    let response = common::response(&app, common::request("/")).await;
    assert!(!response.headers().contains_key("content-encoding"));
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&body).is_ok());
}
