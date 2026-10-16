//! Supported collectors translate failures into the safe HTTP vocabulary.

use crate::ApiError;
use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use rivium::{Error, error::kinds};
use serde::{Serialize, de::DeserializeOwned};

/// JSON collector and response body.
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        crate::response::json_response(self.0, StatusCode::OK)
    }
}

impl<S, T> FromRequest<S> for Json<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(
        request: Request,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(request, state)
            .await
            .map(|value| Self(value.0))
            .map_err(|error| {
                let oversized = error.status() == StatusCode::PAYLOAD_TOO_LARGE;
                #[cfg(feature = "contract")]
                let oversized = oversized
                    || std::iter::successors(
                        Some(&error as &(dyn std::error::Error + 'static)),
                        |cause| cause.source(),
                    )
                    .any(|cause| cause.is::<crate::body::BodyLimitExceeded>());
                ApiError(
                    Error::new(
                        if oversized {
                            kinds::PAYLOAD_TOO_LARGE
                        } else {
                            kinds::INVALID_BODY
                        },
                        "JSON body rejected",
                    )
                    .with_source(error),
                )
            })
    }
}

/// Query-string decoder.
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|value| Self(value.0))
            .map_err(|error| {
                ApiError(Error::invalid("query parameters rejected").with_source(error))
            })
    }
}

/// Typed route-parameter decoder.
pub struct Path<T>(pub T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|value| Self(value.0))
            .map_err(|error| {
                ApiError(Error::invalid("route parameters rejected").with_source(error))
            })
    }
}
