#![cfg(feature = "contract")]
mod common;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use common::request;
use rivium_http::{ContractSettings, NoEnvelope, contract, observer_panics};
use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
fn one() -> ContractSettings {
    ContractSettings {
        in_flight: NonZeroUsize::new(1).unwrap(),
        ..Default::default()
    }
}
type RetainedBody = Arc<Mutex<Option<Body>>>;
async fn retain_request_body(hang: bool) -> (Router, RetainedBody, axum::response::Response) {
    let held = Arc::new(Mutex::new(None));
    let incoming = held.clone();
    let mut settings = one();
    if hang {
        settings.request_timeout_ms = 1;
    }
    let router = contract(
        Router::new()
            .route(
                "/hold",
                post(move |request: Request| {
                    *incoming.lock().unwrap() = Some(request.into_body());
                    async move {
                        if hang {
                            std::future::pending::<()>().await;
                        }
                        "done"
                    }
                }),
            )
            .route("/probe", get(|| async { "free" })),
        &settings,
        vec![],
    );
    let response = common::response(
        &router,
        Request::post("/hold").body(Body::from("retained")).unwrap(),
    )
    .await;
    (router, held, response)
}

#[tokio::test]
async fn transferred_request_body_holds_capacity_after_response_eof() {
    let (router, held, response) = retain_request_body(false).await;
    assert_eq!(to_bytes(response.into_body(), 100).await.unwrap(), "done");
    let full = common::response(&router, request("/probe")).await;
    assert_eq!(full.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(full.headers().contains_key("x-request-id"));
    drop(held.lock().unwrap().take());
    assert_eq!(
        common::response(&router, request("/probe")).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn no_envelope_body_still_holds_capacity_until_drop() {
    let router = contract(
        Router::new().route(
            "/",
            get(|| async {
                let mut response = (StatusCode::BAD_REQUEST, "custom body").into_response();
                response.extensions_mut().insert(NoEnvelope);
                response
            }),
        ),
        &one(),
        vec![],
    );
    let first = common::response(&router, request("/")).await;
    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        common::response(&router, request("/")).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(first);
    assert_eq!(
        common::response(&router, request("/")).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn request_admission_does_not_queue_at_default_capacity() {
    let router = contract(
        Router::new().route("/", get(|| async { "held until drop" })),
        &ContractSettings::default(),
        vec![],
    );
    let mut held = Vec::new();
    for _ in 0..256 {
        let response = common::response(&router, request("/")).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(
        common::response(&router, request("/")).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    held.pop();
    assert_eq!(
        common::response(&router, request("/")).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn panicking_observer_preserves_the_business_response() {
    let panics = observer_panics();
    let called = Arc::new(AtomicBool::new(false));
    let recorded = called.clone();
    let router = contract(
        Router::new().route("/", get(|| async { "success" })),
        &one(),
        vec![
            Arc::new(|_| panic!("observer fault")),
            Arc::new(move |_| {
                recorded.store(true, Ordering::Release);
            }),
        ],
    );
    let response = common::response(&router, request("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap(),
        "success"
    );
    assert!(called.load(Ordering::Acquire));
    assert_eq!(observer_panics(), panics + 1);
}

#[tokio::test]
async fn header_timeout_keeps_transferred_body_owned_until_its_real_release() {
    let (router, held, response) = retain_request_body(true).await;
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    drop(response);
    assert_eq!(
        common::response(&router, request("/probe")).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(held.lock().unwrap().take());
    assert_eq!(
        common::response(&router, request("/probe")).await.status(),
        StatusCode::OK
    );
}

#[tokio::test(start_paused = true)]
async fn response_stream_is_not_subject_to_the_header_deadline() {
    let router = contract(
        Router::new().route(
            "/",
            get(|| async {
                Body::from_stream(futures_util::stream::once(async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<_, std::io::Error>("late body")
                }))
            }),
        ),
        &ContractSettings {
            request_timeout_ms: 1,
            ..one()
        },
        vec![],
    );
    let response = common::response(&router, request("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap(),
        "late body"
    );
}

#[tokio::test]
async fn unknown_unread_suffix_does_not_claim_to_be_validated() {
    let router = contract(
        Router::new().route("/", post(|| async { "ignored body" })),
        &ContractSettings {
            body_limit: 1,
            ..one()
        },
        vec![],
    );
    let body = Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(
        "unread oversized suffix",
    )]));
    let response = common::response(&router, Request::post("/").body(body).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn terminal_body_poll_drops_inner_resources_before_releasing_capacity() {
    use http_body::{Body as HttpBody, Frame};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    struct Ending {
        dropped: Arc<AtomicBool>,
        error: bool,
    }
    impl Drop for Ending {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    impl HttpBody for Ending {
        type Data = axum::body::Bytes;
        type Error = std::io::Error;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(if self.error {
                Some(Err(std::io::Error::other("terminal")))
            } else {
                None
            })
        }
    }
    for error in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = dropped.clone();
        let router = contract(
            Router::new()
                .route(
                    "/end",
                    get(move || {
                        let dropped = marker.clone();
                        async move { Body::new(Ending { dropped, error }) }
                    }),
                )
                .route("/probe", get(|| async { "free" })),
            &one(),
            vec![],
        );
        let mut body = common::response(&router, request("/end")).await.into_body();
        assert!(!dropped.load(Ordering::Acquire));
        let frame = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
        assert_eq!(frame.is_some(), error);
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(
            common::response(&router, request("/probe")).await.status(),
            StatusCode::OK
        );
        // Keep the outer body alive through the probe: its terminal poll released the lease.
        drop(body);
    }
}
