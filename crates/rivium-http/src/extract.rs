//! Extractors that answer their failures with the status envelope: a body that is not the
//! expected JSON is a 406, a query or path that does not parse is a 400, and the description
//! says where and why.

use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{self, FromRequest, FromRequestParts, Request};
use axum::http::header;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use rivium::Error;
use rivium::error::{ErrorType, kinds};
use serde::de::DeserializeOwned;

use crate::ApiError;

/// A JSON body, read into `T`. The request must say `Content-Type: application/json` (or
/// another `application/…+json`); a body that does not parse into `T` is a 406 that names the
/// field, the line and the column, and a body larger than `http.body_limit` is a 413.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Json<T>(pub T);

/// The query string, read into `T`; failing is a 400.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

/// The parameters of the route's path, read into `T`; failing is a 400.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        let content_type = request.headers().get(header::CONTENT_TYPE);
        let mime = content_type.and_then(|value| value.to_str().ok());
        let mime = mime
            .and_then(|mime| mime.split(';').next())
            .unwrap_or_default();
        let mime = mime.trim().to_ascii_lowercase();
        let json = mime == "application/json"
            || mime.starts_with("application/") && mime.ends_with("+json");
        if !json {
            let why = "expected a JSON body, with Content-Type: application/json";
            return Err(failed(kinds::INVALID_BODY, why.to_string()));
        }
        // Too large a body keeps the framework's 413.
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let mut json = serde_json::Deserializer::from_slice(&bytes);
        let value = serde_path_to_error::deserialize(&mut json).map_err(|error| {
            let path = error.path().to_string();
            match path.as_str() {
                "." => error.into_inner().to_string(),
                _ => format!("{path}: {}", error.into_inner()),
            }
        });
        match value.and_then(|value| json.end().map(|()| value).map_err(|e| e.to_string())) {
            Ok(value) => Ok(Json(value)),
            Err(why) => Err(failed(kinds::INVALID_BODY, why)),
        }
    }
}

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        match extract::Query::from_request_parts(parts, state).await {
            Ok(extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => Err(failed(kinds::INVALID_INPUT, rejection.body_text())),
        }
    }
}

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for Path<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        match extract::Path::from_request_parts(parts, state).await {
            Ok(extract::Path(value)) => Ok(Path(value)),
            Err(PathRejection::FailedToDeserializePathParams(rejection)) => {
                Err(failed(kinds::INVALID_INPUT, rejection.body_text()))
            }
            // A route without the parameters: a defect, answered with 500.
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}

fn failed(kind: ErrorType, why: String) -> Response {
    ApiError(Error::explain(kind, why)).into_response()
}
