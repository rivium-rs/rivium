mod common;
use axum::http::StatusCode;

#[test]
fn upstream_input_errors_are_server_errors() {
    let error = rivium::Error::invalid("bad upstream response").upstream();
    assert_eq!(
        rivium_http::status_of(&error),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn only_explicit_caller_text_can_cross_the_http_boundary() {
    use rivium::{
        Error,
        error::{Class, ErrorKind},
    };
    use rivium_http::ApiError;
    const AUTH: &ErrorKind = &ErrorKind::new("Auth", Class::Unauthenticated);
    const DENIED: &ErrorKind = &ErrorKind::new("Deny", Class::Forbidden);
    for error in [
        Error::invalid("secret"),
        Error::invalid("secret")
            .upstream()
            .with_public_message("secret"),
        Error::internal("secret").with_public_message("secret"),
        Error::new(AUTH, "secret").with_public_message("secret"),
        Error::new(DENIED, "secret").with_public_message("secret"),
    ] {
        let body = common::body_text(ApiError(error), 8192).await;
        assert!(!body.contains("secret"));
    }
    let body = common::body_text(
        ApiError(Error::invalid("secret").with_public_message("station missing")),
        8192,
    )
    .await;
    assert!(body.contains("station missing"));
}

#[tokio::test]
async fn serialization_failure_is_private_and_cannot_become_a_created_success() {
    use axum::response::IntoResponse;
    use rivium_http::{ApiResponse, Json};
    struct Broken;
    impl serde::Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("credential=secret"))
        }
    }
    for response in [
        ApiResponse::Data(Broken).into_response(),
        ApiResponse::Created(Broken).into_response(),
        Json(Broken).into_response(),
    ] {
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = common::body_text(response, 4096).await;
        assert!(!body.contains("credential"));
    }
}

#[test]
fn transport_categories_keep_their_declared_statuses() {
    use rivium::{Error, error::kinds};
    let cases = [
        (kinds::INVALID_INPUT, 400),
        (kinds::INVALID_BODY, 406),
        (kinds::PAYLOAD_TOO_LARGE, 413),
        (kinds::UNAUTHENTICATED, 401),
        (kinds::FORBIDDEN, 403),
        (kinds::NOT_FOUND, 404),
        (kinds::CONFLICT, 409),
        (kinds::TOO_MANY_REQUESTS, 429),
        (kinds::UNAVAILABLE, 503),
        (kinds::TIMEOUT, 504),
        (kinds::INTERNAL, 500),
    ];
    for (kind, status) in cases {
        assert_eq!(
            rivium_http::status_of(&Error::new(kind, "private")).as_u16(),
            status
        );
    }
}

#[tokio::test]
async fn successful_shapes_and_retry_hint_remain_explicit() {
    use axum::{body::to_bytes, response::IntoResponse};
    use rivium::{
        Error,
        error::{Retry, kinds},
    };
    use rivium_http::{ApiError, ApiResponse};
    let success = ApiResponse::<()>::Ok.into_response();
    assert_eq!(
        to_bytes(success.into_body(), 4096).await.unwrap(),
        r#"{"status":"success","code":200,"description":""}"#
    );
    let created = ApiResponse::Created(serde_json::json!({"id": 7})).into_response();
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(
        to_bytes(created.into_body(), 4096).await.unwrap(),
        r#"{"id":7}"#
    );
    let retry = ApiError(
        Error::new(kinds::UNAVAILABLE, "private")
            .with_retry(Retry::After(std::time::Duration::from_millis(1500))),
    )
    .into_response();
    assert_eq!(retry.headers()["retry-after"], "2");
}

#[tokio::test]
async fn typed_query_and_path_rejections_keep_details_private_without_middleware() {
    use axum::{Router, routing::get};
    use rivium_http::{Path, Query};
    #[derive(serde::Deserialize)]
    struct Page {
        page: u64,
    }
    let router = Router::new()
        .route(
            "/query",
            get(|Query(page): Query<Page>| async move { page.page.to_string() }),
        )
        .route(
            "/path/{id}",
            get(|Path(id): Path<u64>| async move { id.to_string() }),
        );
    for path in ["/query?page=secret", "/path/secret"] {
        let response = common::response(&router, common::request(path)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = common::body_text(response, 4096).await;
        assert!(!body.contains("secret"));
    }
}
