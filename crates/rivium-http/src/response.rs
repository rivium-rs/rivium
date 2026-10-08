//! Responses: the status envelope, the status of an error, and file downloads.

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rivium::error::{Class, ErrorSource, kinds};
use rivium::{BError, Error};
use serde::Serialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// A failed request, answered with the status envelope: the status is [`status_of`] the error,
/// and the error is logged once, in the access log line. Rivium results turn into it with `?`.
#[derive(Debug)]
pub struct ApiError(pub BError);

impl From<BError> for ApiError {
    fn from(error: BError) -> Self {
        ApiError(error)
    }
}

/// A successful answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiResponse<T = ()> {
    /// 200 with the envelope `{"status":"success","code":200,"description":""}`.
    Ok,
    /// 200 with the data as JSON.
    Data(T),
    /// 201 with the data as JSON.
    Created(T),
}

/// What a handler returns.
pub type ApiResult<T = ()> = Result<ApiResponse<T>, ApiError>;

/// The HTTP status of an error, by its class. A client error that a dependency caused
/// ([`ErrorSource::Upstream`]) is not the caller's fault: it becomes 500, or 503 for
/// [`Class::TooManyRequests`].
///
/// | Class | Status | Upstream |
/// | --- | --- | --- |
/// | `InvalidInput` | 400 | 500 |
/// | `InvalidBody` | 406 | 500 |
/// | `Unauthenticated` | 401 | 500 |
/// | `Forbidden` | 403 | 500 |
/// | `NotFound` | 404 | 500 |
/// | `Conflict` | 409 | 500 |
/// | `TooManyRequests` | 429 | 503 |
/// | `Unavailable` | 503 | 503 |
/// | `Timeout` | 504 | 504 |
/// | `Internal` | 500 | 500 |
#[must_use]
pub fn status_of(error: &Error) -> StatusCode {
    let (own, upstream) = match error.class() {
        Class::InvalidInput => (400, 500),
        Class::InvalidBody => (406, 500),
        Class::Unauthenticated => (401, 500),
        Class::Forbidden => (403, 500),
        Class::NotFound => (404, 500),
        Class::Conflict => (409, 500),
        Class::TooManyRequests => (429, 503),
        Class::Unavailable => (503, 503),
        Class::Timeout => (504, 504),
        _ => (500, 500),
    };
    let code = match error.esource() {
        ErrorSource::Upstream => upstream,
        _ => own,
    };
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// The error behind a response, for the envelope and the access log.
#[derive(Clone)]
pub(crate) struct Failed(pub(crate) Arc<Error>);

/// Marks a response that keeps its body whatever its status, as the probes' do.
#[derive(Clone, Copy)]
pub(crate) struct Plain;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = status_of(&self.0).into_response();
        response.extensions_mut().insert(Failed(Arc::from(self.0)));
        // The server's middleware renders it again with the request id.
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
pub(crate) fn json(status: StatusCode, value: &impl Serialize) -> Response {
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

/// Turns an error response into the status envelope: one from an [`ApiError`], with the error's
/// description, or another response of 400 and above from the framework or the router's
/// services, with a fixed one. Other responses pass unchanged; headers other than the body's
/// stay.
pub(crate) fn render(response: Response, request_id: Option<&str>, expose: bool) -> Response {
    let status = response.status();
    let failed = response.extensions().get::<Failed>().cloned();
    let error = failed.as_ref().map(|failed| &*failed.0);
    let plain = response.extensions().get::<Plain>().is_some();
    if error.is_none() && (status.as_u16() < 400 || plain) {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    let mut rendered = envelope(status, &describe(error, status, request_id, expose));
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.extend(rendered.headers_mut().drain());
    if error.is_some_and(|error| error.retry()) && matches!(status.as_u16(), 429 | 503) {
        parts
            .headers
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    Response::from_parts(parts, rendered.into_body())
}

/// What the envelope says: for a server error a fixed phrase and the request id (or the whole
/// error chain when `expose`), for a client error the error's outermost context.
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

/// Serves the file at `path` to download as `file_name`, as tower-http's `ServeFile` does:
/// with ranges, `HEAD` and `If-Modified-Since`, and with `Content-Disposition: attachment`. A
/// file that is not there is a 404.
pub async fn file_response(request: Request, path: &Path, file_name: &str) -> Response {
    let Ok(response) = ServeFile::new(path).oneshot(request).await;
    let mut response = response.map(Body::new);
    if response.status().is_success()
        && let Ok(value) = HeaderValue::from_str(&disposition(file_name))
    {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

/// `attachment; filename="…"`, and `filename*` (RFC 6266) for a name that is not plain ASCII.
fn disposition(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| match c {
            ' '..='~' if c != '"' && c != '\\' => c,
            _ => '_',
        })
        .collect();
    if plain == name {
        return format!("attachment; filename=\"{name}\"");
    }
    let encoded: String = (name.bytes())
        .map(|b| match b {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use rivium::error::{Class, Error, ErrorKind, ErrorType, kinds};

    use super::{describe, disposition, status_of};

    #[test]
    fn statuses_follow_the_class_and_whose_fault_it_is() {
        let cases = [
            (Class::InvalidInput, 400, 500),
            (Class::InvalidBody, 406, 500),
            (Class::Unauthenticated, 401, 500),
            (Class::Forbidden, 403, 500),
            (Class::NotFound, 404, 500),
            (Class::Conflict, 409, 500),
            (Class::TooManyRequests, 429, 503),
            (Class::Unavailable, 503, 503),
            (Class::Timeout, 504, 504),
            (Class::Internal, 500, 500),
        ];
        for (class, own, upstream) in cases {
            let kind: ErrorType = Box::leak(Box::new(ErrorKind::new("Kind", class)));
            let error = Error::explain(kind, "context");
            assert_eq!(status_of(&error).as_u16(), own, "{class}");
            assert_eq!(status_of(&error.into_up()).as_u16(), upstream, "{class}");
            let downstream = Error::explain(kind, "context").into_down();
            assert_eq!(status_of(&downstream).as_u16(), own, "{class}");
        }
    }

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

    #[test]
    fn download_names_are_quoted_and_encoded() {
        assert_eq!(
            disposition("logs 2026-10-08.zip"),
            "attachment; filename=\"logs 2026-10-08.zip\""
        );
        assert_eq!(
            disposition("日志\".zip"),
            "attachment; filename=\"___.zip\"; filename*=UTF-8''%E6%97%A5%E5%BF%97%22.zip"
        );
    }
}
