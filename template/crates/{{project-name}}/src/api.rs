//! A sample HTTP API, to replace with the business: `GET /hello`. Handlers answer with
//! `ApiResult`: data as JSON, and errors as the status envelope with the status of their class.
//! AGENTS.md, "HTTP", says where layers go, how a response keeps a body of its own, and how long
//! responses end at the stop.

use axum::Router;
use axum::routing::get;
use rivium_http::{ApiResponse, ApiResult};
use serde::Serialize;

/// The API's routes. `HttpServer` adds the probes `/livez` and `/readyz` and puts them all in
/// the contract layer; layers added here run inside it.
pub fn router() -> Router {
    Router::new().route("/hello", get(hello))
}

/// The answer of `GET /hello`.
#[derive(Serialize)]
struct Hello {
    message: &'static str,
}

async fn hello() -> ApiResult<Hello> {
    Ok(ApiResponse::Data(Hello { message: "hello" }))
}
