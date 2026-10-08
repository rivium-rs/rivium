//! axum integration for Rivium services.
//!
//! [`HttpServer`] runs an axum `Router` as a frontline [`Service`](rivium::Service), with the
//! settings of an `[http]` section ([`HttpSettings`]): it serves the probes `/livez` and
//! `/readyz`, gives every request an id and a span, logs one access line per request, and
//! answers failures with a fixed status envelope:
//!
//! | Response | Status | Body |
//! | --- | --- | --- |
//! | [`ApiResponse::Ok`] | 200 | `{"status":"success","code":200,"description":""}` |
//! | [`ApiResponse::Data`], [`ApiResponse::Created`] | 200, 201 | the data as JSON |
//! | [`ApiError`], and errors of the framework such as an unknown route | [`status_of`] the error | `{"status":"error","code":<status>,"description":"…"}` |
//!
//! A client error's description is the error's outermost context; a server error's is a fixed
//! phrase with the request id, such as `internal error (request_id=…)`, and the error itself
//! goes to the log. Every response carries the header `x-request-id`.
//!
//! ```no_run
//! use axum::Router;
//! use axum::routing::get;
//! use rivium::error::{Error, kinds};
//! use rivium_http::extract::Path;
//! use rivium_http::{ApiResponse, ApiResult, HttpServer, HttpSettings};
//!
//! async fn device(Path(id): Path<u32>) -> ApiResult<String> {
//!     match id {
//!         7 => Ok(ApiResponse::Data("printer".to_string())),
//!         _ => Err(Error::explain(kinds::NOT_FOUND, format!("no device {id}")).into()),
//!     }
//! }
//!
//! fn services(
//!     http: &HttpSettings,
//!     ctx: &rivium::AppContext,
//! ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
//!     let router = Router::new().route("/devices/{id}", get(device));
//!     Ok(vec![Box::new(HttpServer::new(http, router, ctx))])
//! }
//! ```
#![forbid(unsafe_code)]

mod access;
pub mod extract;
mod response;
mod server;

pub use access::{HttpObserver, ResponseInfo};
pub use response::{ApiError, ApiResponse, ApiResult, file_response, status_of};
pub use server::{HttpServer, HttpSettings};
