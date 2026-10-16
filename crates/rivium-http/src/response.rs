#[cfg(feature = "contract")]
use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, request::Parts},
};
use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rivium::{
    Error,
    error::{Class, Retry, Side},
};
use serde::Serialize;
#[cfg(feature = "contract")]
use std::convert::Infallible;

/// Standard successful response shapes. Data responses are bare JSON.
pub enum ApiResponse<T> {
    /// Empty successful envelope.
    Ok,
    /// JSON data with status 200.
    Data(T),
    /// JSON data with status 201.
    Created(T),
}
/// Handler result using the standard HTTP error adapter.
pub type ApiResult<T = ()> = std::result::Result<ApiResponse<T>, ApiError>;
/// HTTP adapter retaining the original private diagnostic error in extensions.
#[derive(Clone, Debug)]
pub struct ApiError(pub Error);
impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}
/// Preserve an application's response body instead of applying the default envelope.
#[derive(Clone, Copy, Debug)]
pub struct NoEnvelope;

#[derive(Serialize)]
struct Envelope<'a> {
    status: &'a str,
    code: u16,
    description: &'a str,
}
impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        match self {
            Self::Ok => axum::Json(Envelope {
                status: "success",
                code: 200,
                description: "",
            })
            .into_response(),
            Self::Data(data) => json_response(data, StatusCode::OK),
            Self::Created(data) => json_response(data, StatusCode::CREATED),
        }
    }
}
pub(crate) fn json_response(data: impl Serialize, status: StatusCode) -> Response {
    match serde_json::to_vec(&data) {
        Ok(bytes) => (
            status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            bytes,
        )
            .into_response(),
        Err(error) => {
            ApiError(Error::internal("response JSON serialization failed").with_source(error))
                .into_response()
        }
    }
}
/// Map a transport-independent failure without treating upstream input as caller error.
pub fn status_of(error: &Error) -> StatusCode {
    if error.side() == Side::Upstream {
        return match error.class() {
            Class::TooManyRequests | Class::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Class::Timeout => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
    }
    match error.class() {
        Class::InvalidInput => StatusCode::BAD_REQUEST,
        Class::InvalidBody => StatusCode::NOT_ACCEPTABLE,
        Class::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        Class::Unauthenticated => StatusCode::UNAUTHORIZED,
        Class::Forbidden => StatusCode::FORBIDDEN,
        Class::NotFound => StatusCode::NOT_FOUND,
        Class::Conflict => StatusCode::CONFLICT,
        Class::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        Class::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Class::Timeout => StatusCode::GATEWAY_TIMEOUT,
        Class::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = status_of(&self.0).into_response();
        if let Retry::After(after) = self.0.retry() {
            let seconds = after
                .as_secs()
                .saturating_add(u64::from(after.subsec_nanos() != 0));
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response.extensions_mut().insert(self.0);
        render(response, None)
    }
}
/// Clone the original diagnostic error attached by `ApiError`.
pub fn error_of(response: &Response) -> Option<Error> {
    response.extensions().get::<Error>().cloned()
}
/// Apply a safe envelope to errors, preserving status, headers, and extensions.
/// Private message/context/source text is never selected for disclosure.
pub fn render(mut response: Response, request_id: Option<&str>) -> Response {
    let status = response.status();
    if response.extensions().get::<NoEnvelope>().is_some()
        || !(status.is_client_error() || status.is_server_error())
    {
        return response;
    }
    let error = error_of(&response);
    let allowed = status.is_client_error()
        && status != StatusCode::UNAUTHORIZED
        && status != StatusCode::FORBIDDEN;
    let mut description = match error.as_ref() {
        Some(error) if allowed && error.side() == Side::Caller => error
            .public_message()
            .or(error.kind().title)
            .unwrap_or(status.canonical_reason().unwrap_or("HTTP error"))
            .to_owned(),
        _ => status.canonical_reason().unwrap_or("HTTP error").to_owned(),
    };
    if status.is_server_error()
        && let Some(id) = request_id
    {
        description.push_str(" (request_id=");
        description.push_str(id);
        description.push(')');
    }
    let envelope = axum::Json(Envelope {
        status: "error",
        code: status.as_u16(),
        description: &description,
    })
    .into_response();
    *response.body_mut() = envelope.into_body();
    response.headers_mut().remove(header::CONTENT_LENGTH);
    response.headers_mut().remove(header::CONTENT_ENCODING);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// Explicit request correlation value; the contract validates it before transport use.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);
impl RequestId {
    /// Correlation header name.
    pub const HEADER: &'static str = "x-request-id";
    /// Supply an explicit opaque ID; this constructor does not generate identifiers.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    #[cfg(feature = "contract")]
    pub(crate) fn validated(value: &str) -> Option<Self> {
        (!value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| (33..=126).contains(&byte)))
        .then(|| Self(value.to_owned()))
    }
    /// Read a valid incoming header, or generate UUIDv7. Requires `contract`.
    #[cfg(feature = "contract")]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        headers
            .get(Self::HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(Self::validated)
            .unwrap_or_default()
    }
    /// Opaque text; the contract validates externally supplied values.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[cfg(feature = "contract")]
impl Default for RequestId {
    fn default() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}
impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
#[cfg(feature = "contract")]
impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = Infallible;
    async fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Self>()
            .map(|id| Self::validated(id.as_str()).unwrap_or_default())
            .unwrap_or_else(|| Self::from_headers(&parts.headers)))
    }
}
