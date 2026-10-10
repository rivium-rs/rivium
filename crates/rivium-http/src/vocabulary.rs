//! The vocabulary: what handlers return, the status of an error, and file downloads. The
//! responses of [`ApiResponse`] and [`ApiError`] are written by the primitives
//! (`primitives.rs`), which give an error response the status envelope and the error that the
//! contract layer logs.

use std::path::Path;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use rivium::error::{Class, ErrorSource};
use rivium::{BError, Error};
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
    use rivium::error::{Class, Error, ErrorKind, ErrorType};

    use super::{disposition, status_of};

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
