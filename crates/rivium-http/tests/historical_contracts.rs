#![cfg(feature = "contract")]
mod common;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use rivium::{Error, error::kinds};
use rivium_http::{ApiError, ContractSettings, NoEnvelope, RequestId, contract, error_of, render};
use std::{
    collections::VecDeque,
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};

#[test]
fn upstream_throttling_remains_service_unavailable() {
    assert_eq!(
        rivium_http::status_of(
            &Error::new(kinds::TOO_MANY_REQUESTS, "dependency quota").upstream()
        ),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
struct Frames(VecDeque<Frame<Bytes>>);
impl HttpBody for Frames {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        Poll::Ready(self.get_mut().0.pop_front().map(Ok))
    }
    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(
            self.0
                .iter()
                .filter_map(|frame| frame.data_ref())
                .map(|data| data.len() as u64)
                .sum(),
        )
    }
}
#[tokio::test]
async fn response_ownership_wrappers_preserve_data_trailers_and_size_hint() {
    let app = contract(
        Router::new().route(
            "/",
            get(|| async {
                let mut trailers = HeaderMap::new();
                trailers.insert("checksum", HeaderValue::from_static("verified"));
                Body::new(Frames(VecDeque::from([
                    Frame::data(Bytes::from_static(b"data")),
                    Frame::trailers(trailers),
                ])))
            }),
        ),
        &ContractSettings::default(),
        vec![],
    );
    let mut body = common::response(&app, Request::new(Body::empty()))
        .await
        .into_body();
    assert_eq!(body.size_hint().exact(), Some(4));
    let data = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert_eq!(data, "data");
    assert_eq!(body.size_hint().exact(), Some(0));
    let trailers = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap()
        .into_trailers()
        .unwrap();
    assert_eq!(trailers["checksum"], "verified");
    assert!(
        futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .is_none()
    );
}
async fn primitives(mut request: Request, next: Next) -> Response {
    let id = RequestId::from_headers(request.headers());
    request.extensions_mut().insert(id.clone());
    let mut response = render(next.run(request).await, Some(id.as_str()));
    response.headers_mut().insert(
        RequestId::HEADER,
        HeaderValue::from_str(id.as_str()).unwrap(),
    );
    response
}
#[tokio::test]
async fn public_render_and_request_id_compose_without_the_contract_layer() {
    let app = Router::new()
        .route(
            "/error",
            get(|| async { ApiError(Error::invalid("private").with_public_message("safe")) }),
        )
        .route(
            "/raw",
            get(|| async {
                let mut response = (StatusCode::BAD_REQUEST, "custom").into_response();
                response.extensions_mut().insert(NoEnvelope);
                response
            }),
        )
        .layer(middleware::from_fn(primitives));
    let response = common::response(
        &app,
        Request::builder()
            .uri("/error")
            .header("x-request-id", "caller-id")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.headers()["x-request-id"], "caller-id");
    assert_eq!(error_of(&response).unwrap().message(), "private");
    let body = common::body_text(response, 4096).await;
    assert!(body.contains("safe"));
    assert!(!body.contains("private"));
    let raw = common::response(&app, common::request("/raw")).await;
    assert_eq!(to_bytes(raw.into_body(), 4096).await.unwrap(), "custom");
}
#[cfg(feature = "fs")]
#[tokio::test]
async fn download_names_encode_quotes_unicode_and_header_control_bytes() {
    let directory = rivium_test::TempDir::new().unwrap();
    let file = directory.path().join("data");
    std::fs::write(&file, "data").unwrap();
    let response = rivium_http::file_response(
        Request::new(Body::empty()),
        file,
        "日志\".zip\r\nX-Injected: yes",
    )
    .await;
    let value = response.headers()["content-disposition"].to_str().unwrap();
    for encoded in ["%E6%97%A5", "%22", "%0D%0A"] {
        assert!(value.contains(encoded), "{value}");
    }
    assert!(!value.contains('\r') && !value.contains('\n'));
    assert!(!response.headers().contains_key("x-injected"));
}

#[cfg(feature = "server")]
mod server_contracts {
    use super::*;
    use rivium::{Kind, StopReason, Supervisor};
    use rivium_http::{HttpServer, ServerLimits, serve_listener};
    use std::{
        io,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
    };
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
    struct ObservedIo {
        inner: DuplexStream,
        dropped: Arc<AtomicBool>,
    }
    impl Drop for ObservedIo {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    impl AsyncRead for ObservedIo {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_read(cx, buffer)
        }
    }
    impl AsyncWrite for ObservedIo {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.get_mut().inner).poll_write(cx, buffer)
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_flush(cx)
        }
        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
        }
    }
    struct OneConnection(Option<ObservedIo>);
    impl axum::serve::Listener for OneConnection {
        type Io = ObservedIo;
        type Addr = ();
        async fn accept(&mut self) -> (ObservedIo, ()) {
            match self.0.take() {
                Some(io) => (io, ()),
                None => std::future::pending().await,
            }
        }
        fn local_addr(&self) -> io::Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn a_transferred_request_body_keeps_the_connection_lease_after_io_is_dropped() {
        let (server_io, mut client) = tokio::io::duplex(4096);
        let dropped = Arc::new(AtomicBool::new(false));
        let held = Arc::new(Mutex::new(None));
        let captured = held.clone();
        let scope = Arc::new(OnceLock::new());
        let router = Router::new().route(
            "/",
            axum::routing::post(move |request: Request| {
                *captured.lock().unwrap() = Some(request.into_body());
                async { "ok" }
            }),
        );
        let registered = scope.clone();
        let listener = OneConnection(Some(ObservedIo {
            inner: server_io,
            dropped: dropped.clone(),
        }));
        let supervisor = Supervisor::default().with(rivium::service(
            "fixture",
            Kind::Frontline,
            move |context| {
                assert!(registered.set(context.task_scope()).is_ok());
                serve_listener(listener, router, context, ServerLimits::default())
            },
        ));
        let (control, _readiness, run) = common::ready(supervisor).await;
        client
            .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata")
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.read_to_string(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            scope.get().unwrap().active_tasks(),
            1,
            "the transferred body still owns the closed connection"
        );
        let body = held.lock().unwrap().take();
        drop(body);
        assert_eq!(scope.get().unwrap().active_tasks(), 0);
        control.stop(StopReason::Requested);
        assert!(run.await.unwrap().drain.is_complete());
    }
    async fn outer(mut request: Request, next: Next) -> Response {
        request.extensions_mut().insert(RequestId::new("outer"));
        let mut response = next.run(request).await;
        response
            .headers_mut()
            .insert("x-outer", HeaderValue::from_static("seen"));
        response
    }
    #[tokio::test]
    async fn outer_layers_and_observers_remain_isolated_per_server() {
        let (first, first_address) = common::bind().await;
        let (second, second_address) = common::bind().await;
        let one = Arc::new(Mutex::new(Vec::new()));
        let two = Arc::new(Mutex::new(Vec::new()));
        let observer_one = one.clone();
        let observer_two = two.clone();
        let router =
            Router::new().route("/data", get(|| async { rivium_http::ApiResponse::Data(7) }));
        let a = HttpServer::from_listener("first", first, router.clone())
            .unwrap()
            .observe(Arc::new(move |access| {
                observer_one.lock().unwrap().push(access.clone())
            }))
            .outer(|router| router.layer(middleware::from_fn(outer)));
        let b = HttpServer::from_listener("second", second, router)
            .unwrap()
            .observe(Arc::new(move |access| {
                observer_two.lock().unwrap().push(access.clone())
            }));
        let supervisor = Supervisor::default().with(Box::new(a)).with(Box::new(b));
        let (control, _readiness, run) = common::ready(supervisor).await;
        let first = common::call(first_address, "/data", Some("caller")).await;
        let second = common::call(second_address, "/data", Some("caller")).await;
        assert!(first.contains("x-outer: seen"));
        assert!(first.contains("x-request-id: outer"));
        assert!(!second.contains("x-outer:"));
        assert!(second.contains("x-request-id: caller"));
        for (list, id) in [(&one, "outer"), (&two, "caller")] {
            let list = list.lock().unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].request_id, id);
            assert_eq!(list[0].method, "GET");
            assert_eq!(list[0].path, "/data");
            assert_eq!(list[0].status, 200);
        }
        control.stop(StopReason::Requested);
        assert!(run.await.unwrap().drain.is_complete());
    }
}

#[tokio::test]
async fn invalid_and_absent_request_ids_generate_uuid_v7_and_valid_ids_are_preserved() {
    let app =
        contract(
            Router::new().route(
                "/",
                get(|id: RequestId| async move {
                    rivium_http::ApiResponse::Data(id.as_str().to_owned())
                }),
            ),
            &ContractSettings::default(),
            vec![],
        );
    let mut generated = std::collections::BTreeSet::new();
    for supplied in [
        None,
        Some("".into()),
        Some("has space".into()),
        Some("x".repeat(129)),
    ] {
        let mut request = Request::builder().uri("/");
        if let Some(value) = supplied {
            request = request.header("x-request-id", value);
        }
        let response = common::response(&app, request.body(Body::empty()).unwrap()).await;
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(id.len(), 36, "{id}");
        assert_eq!(id.as_bytes()[14], b'7');
        assert!(generated.insert(id.clone()));
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), id);
    }
    let valid = "x".repeat(128);
    let response = common::response(
        &app,
        Request::builder()
            .uri("/")
            .header("x-request-id", &valid)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.headers()["x-request-id"], valid);
}

#[tokio::test]
async fn framework_error_envelopes_preserve_the_allow_header() {
    let app = contract(
        Router::new().route("/", get(|| async { "ok" })),
        &ContractSettings::default(),
        vec![],
    );
    let response = common::response(&app, Request::delete("/").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["allow"], "GET,HEAD");
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        405
    );
}
