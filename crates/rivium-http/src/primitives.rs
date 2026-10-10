//! The primitives: the parts that keep the contract, public so that middleware of a server's own
//! can keep it as the contract layer does. [`render`] writes the status envelope of an error
//! response, [`error_of`] reads the error behind a response, [`RequestId`] is a request's id,
//! and [`NoEnvelope`] marks a response that keeps its own body. The vocabulary's responses are
//! written here too: an [`ApiError`] leaves its error on its response for `render` and
//! `error_of`.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, IntoResponseParts, Response, ResponseParts};
use rivium::Error;
use rivium::error::{Class, kinds};
use serde::Serialize;
use uuid::Uuid;

use crate::{ApiError, ApiResponse, status_of};

/// The error behind a response.
#[derive(Clone)]
struct Failed(Arc<Error>);

/// Marks a response of 400 and above that keeps its own body and headers instead of the status
/// envelope, such as an error format that older clients expect:
/// `(StatusCode::CONFLICT, NoEnvelope, Json(..))` in a handler, or
/// `response.extensions_mut().insert(NoEnvelope)` in middleware. Such a response carries no
/// error, so its access log line has the level of its status and no error fields, and observers
/// see no error. A response that also carries the error of an [`ApiError`] gets the envelope.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoEnvelope;

impl IntoResponseParts for NoEnvelope {
    type Error = Infallible;

    fn into_response_parts(self, mut parts: ResponseParts) -> Result<ResponseParts, Infallible> {
        parts.extensions_mut().insert(self);
        Ok(parts)
    }
}

/// A request's id. The contract layer keeps the id that an outer layer put in the request's
/// extensions, such as one taken from `traceparent`; else the caller's [`x-request-id`] when it
/// is 1 to 128 visible ASCII characters; else a new UUIDv7. It puts the id in the request's
/// extensions and in the span of everything the request logs, and on the response.
///
/// Handlers extract it; without middleware that set one, extracting it is a 500.
///
/// [`x-request-id`]: Self::HEADER
#[derive(Clone, Debug)]
pub struct RequestId(Arc<str>);

impl RequestId {
    /// The header that carries the id: `x-request-id`.
    pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

    /// The caller's id in `headers` when it is 1 to 128 visible ASCII characters, else a new
    /// UUIDv7.
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let given = headers.get(Self::HEADER).map(HeaderValue::as_bytes);
        let valid =
            |id: &&[u8]| (1..=128).contains(&id.len()) && id.iter().all(u8::is_ascii_graphic);
        RequestId(match given.filter(valid) {
            Some(id) => String::from_utf8_lossy(id).into(),
            None => Uuid::now_v7().to_string().into(),
        })
    }

    /// An id by the server's own rules.
    #[must_use]
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        RequestId(id.into())
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        match parts.extensions.get::<RequestId>() {
            Some(id) => Ok(id.clone()),
            None => {
                let why = "the request has no id: no contract layer runs in front of the handler";
                Err(ApiError(Error::explain(kinds::INTERNAL, why)))
            }
        }
    }
}

/// Turns an error response into the status envelope: one that an [`ApiError`] made gets the
/// error's description, and another response of 400 and above, such as one of the framework or
/// of the router's services, a fixed one, unless it carries [`NoEnvelope`]. A server error's
/// description is a fixed phrase with `request_id`, or with `expose_internal_detail` the whole
/// error; a client error's is the error's outermost context. A retryable error's 429 or 503
/// gets `Retry-After: 1`. Other responses pass unchanged, and headers other than the body's
/// stay.
#[must_use]
pub fn render(
    response: Response,
    request_id: Option<&str>,
    expose_internal_detail: bool,
) -> Response {
    let status = response.status();
    let failed = response.extensions().get::<Failed>().cloned();
    let error = failed.as_ref().map(|failed| &*failed.0);
    let own = response.extensions().get::<NoEnvelope>().is_some();
    if error.is_none() && (status.as_u16() < 400 || own) {
        return response;
    }
    let description = describe(error, status, request_id, expose_internal_detail);
    let (mut parts, _) = response.into_parts();
    let mut rendered = envelope(status, &description);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.extend(rendered.headers_mut().drain());
    if error.is_some_and(|error| error.retry()) && matches!(status.as_u16(), 429 | 503) {
        parts
            .headers
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    Response::from_parts(parts, rendered.into_body())
}

/// The error behind a response that an [`ApiError`] made, for the access log and observers.
#[must_use]
pub fn error_of(response: &Response) -> Option<Arc<Error>> {
    let failed = response.extensions().get::<Failed>();
    failed.map(|failed| Arc::clone(&failed.0))
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = status_of(&self.0).into_response();
        response.extensions_mut().insert(Failed(Arc::from(self.0)));
        // The contract layer renders it again with the request id.
        render(response, None, false)
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        match self {
            ApiResponse::Ok => envelope(StatusCode::OK, ""),
            ApiResponse::Data(data) => json(StatusCode::OK, &data),
            ApiResponse::Created(data) => json(StatusCode::CREATED, &data),
        }
    }
}

/// `value` as JSON, with this status.
fn json(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (status, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(error) => {
            ApiError(Error::because(kinds::INTERNAL, "writing JSON", error)).into_response()
        }
    }
}

/// The status envelope, with its three fields in this order.
fn envelope(status: StatusCode, description: &str) -> Response {
    #[derive(Serialize)]
    struct Envelope<'a> {
        status: &'a str,
        code: u16,
        description: &'a str,
    }
    let outcome = if status.is_success() {
        "success"
    } else {
        "error"
    };
    let envelope = Envelope {
        status: outcome,
        code: status.as_u16(),
        description,
    };
    json(status, &envelope)
}

/// What the envelope says: for a server error a fixed phrase and the request id (or the whole
/// error chain when `expose`), for a client error the error's outermost context, except for 401
/// and 403.
fn describe(error: Option<&Error>, status: StatusCode, id: Option<&str>, expose: bool) -> String {
    let title = error.and_then(|error| error.etype().title());
    if status.is_server_error() {
        if let Some(error) = error.filter(|_| expose) {
            return format!("{error:#}");
        }
        let phrase = title.unwrap_or(phrase(status));
        return id.map_or(phrase.to_string(), |id| {
            format!("{phrase} (request_id={id})")
        });
    }
    match error {
        Some(error) if !matches!(error.class(), Class::Unauthenticated | Class::Forbidden) => {
            let context = error.context().or(title);
            context.unwrap_or(error.etype().name()).to_string()
        }
        _ => title.unwrap_or(phrase(status)).to_string(),
    }
}

/// The fixed phrase of a status.
fn phrase(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "unauthenticated",
        403 => "forbidden",
        503 => "service unavailable",
        504 => "timeout",
        500.. => "internal error",
        _ => status.canonical_reason().unwrap_or("error"),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use rivium::error::{Class, Error, ErrorKind, ErrorType, kinds};

    use super::describe;

    #[test]
    fn descriptions_hide_server_errors_and_show_client_errors() {
        const TITLED: ErrorType =
            &ErrorKind::new("TokenExpired", Class::Unauthenticated).titled("token expired");
        let detail = Error::explain(kinds::INTERNAL, "the database password is wrong");
        let cases = [
            (
                Error::explain(kinds::NOT_FOUND, "no device 7"),
                404,
                "no device 7",
            ),
            (Error::new(kinds::CONFLICT), 409, "Conflict"),
            (
                Error::explain(TITLED, "the token of 2026-10-01"),
                401,
                "token expired",
            ),
            (detail, 500, "internal error (request_id=r1)"),
            (
                Error::new(kinds::UNAVAILABLE),
                503,
                "service unavailable (request_id=r1)",
            ),
        ];
        for (error, status, expected) in cases {
            let status = StatusCode::from_u16(status).unwrap();
            assert_eq!(describe(Some(&error), status, Some("r1"), false), expected);
        }
        let error = Error::explain(kinds::INTERNAL, "outer").more_context("outermost");
        let status = StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(
            describe(Some(&error), status, Some("r1"), true),
            "outermost: outer"
        );
        assert_eq!(
            describe(None, StatusCode::NOT_FOUND, Some("r1"), false),
            "Not Found"
        );
        let range = StatusCode::RANGE_NOT_SATISFIABLE;
        assert_eq!(describe(None, range, None, false), "Range Not Satisfiable");
    }
}
