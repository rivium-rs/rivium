//! axum integration for Rivium services: the HTTP contract (a fixed status envelope, the status
//! of each error class, request ids and one access log line per response, with the error logged
//! once) and a server that keeps it as a Rivium service. It comes in four layers, each built on
//! the public API of the layers below, so that a service can stop at any of them:
//!
//! | Layer | Items | Use |
//! | --- | --- | --- |
//! | Vocabulary | [`ApiResponse`], [`ApiError`], [`ApiResult`], [`status_of`], [`extract`], [`file_response`] | what handlers return and extract |
//! | Primitives | [`render`], [`error_of`], [`RequestId`], [`NoEnvelope`] | middleware of a server's own that keeps the contract |
//! | Contract layer | [`contract`], [`ContractSettings`], [`HttpObserver`], [`ResponseInfo`] | the whole contract on any axum router, also in `oneshot` tests |
//! | Server | [`HttpServer`], [`HttpSettings`], [`Acceptor`] | a frontline Rivium service: the contract layer, the probes and the listener, with hooks for outer layers and TLS |
//!
//! The status envelope:
//!
//! | Response | Status | Body |
//! | --- | --- | --- |
//! | [`ApiResponse::Ok`] | 200 | `{"status":"success","code":200,"description":""}` |
//! | [`ApiResponse::Data`], [`ApiResponse::Created`] | 200, 201 | the data as JSON |
//! | [`ApiError`], and errors of the framework such as an unknown route | [`status_of`] the error | `{"status":"error","code":<status>,"description":"…"}` |
//!
//! A client error's description is the error's outermost context; for 401 and 403 it is the
//! error kind's title or a fixed phrase. A server error's is a fixed phrase with the request id,
//! such as `internal error (request_id=…)`, and the error itself goes to the log. A response of
//! 400 and above that carries [`NoEnvelope`] keeps its own body. Responses that pass the
//! contract layer carry the header `x-request-id` and get one access log line each; a request
//! that hyper rejects itself (a malformed request line or head), whose client leaves before the
//! answer, or that is still running at the stop deadline has neither.
//!
//! What each way of using the crate keeps of the contract; what the vocabulary alone leaves out,
//! it leaves out silently:
//!
//! | Contract | Vocabulary | + primitives | Contract layer | `HttpServer` |
//! | --- | --- | --- | --- | --- |
//! | Success shapes, statuses, client error descriptions, extractor errors, `Retry-After` | yes | yes | yes | yes |
//! | Server errors with the request id, `expose_internal_detail`, the envelope for framework errors | no | with `render` | yes | yes |
//! | Errors logged once, the access log, observers | no | with `error_of` | yes | yes |
//! | Request ids made, checked, returned and extracted | no | with `RequestId` | yes | yes |
//! | 503 for a timeout, 500 for a panic, the body limit | no | own code | yes | yes |
//! | Probes | no | own code | own code | yes |
//! | The stop signal and `ConnectInfo` in requests, a TLS hook, layers outside the contract | own code | own code | own code | yes |
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
mod primitives;
mod server;
mod vocabulary;

pub use access::{ContractSettings, HttpObserver, ResponseInfo, contract};
pub use primitives::{NoEnvelope, RequestId, error_of, render};
pub use server::{Acceptor, HttpServer, HttpSettings};
pub use vocabulary::{ApiError, ApiResponse, ApiResult, file_response, status_of};
