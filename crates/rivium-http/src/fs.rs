use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use std::path::Path;
use tower::ServiceExt;

/// Serve an already-authorized path with Tower's Range/HEAD behavior.
/// Authorization and mapping untrusted path input remain the application's responsibility.
pub async fn file_response(
    request: Request,
    path: impl AsRef<Path>,
    download_name: &str,
) -> Response {
    let served = tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await;
    let mut response = match served {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };
    if response.status().is_success() {
        let mut encoded = String::new();
        for byte in download_name.as_bytes() {
            if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(byte) {
                encoded.push(char::from(*byte));
            } else {
                use std::fmt::Write;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}"))
        {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
    }
    response.extensions_mut().insert(crate::NoEnvelope);
    response.into_response()
}
