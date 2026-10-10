//! Log export over HTTP: the binding of Rivium's [`LogExporter`] to the protocol that this
//! service's clients speak. Rivium chooses, packs, cancels and expires the archives; the
//! binding is what each service writes for itself: the paths, the field names, and the kinds
//! of logs the clients know, which it maps to log files.
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET /api/logs/kinds` | `[{"kind":"service"},{"kind":"access"}]`: the kinds there are files for |
//! | `POST /api/logs/exports` with `{"from":"2026-10-01","to":"2026-10-08","kinds":["service"]}` | 201 `{"id":3}`; all kinds when `kinds` is left out; 400 `{"unknown":["audit"],"kinds":["service","access"]}` for kinds the binding does not know |
//! | `GET /api/logs/exports/{id}` | `{"percent":40,"state":"running"}`; `"error"` too once failed |
//! | `DELETE /api/logs/exports/{id}` | cancels the export, or removes its archive |
//! | `GET /api/logs/exports/{id}/archive` | the zip archive, which can be downloaded in ranges |

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rivium::log::{Date, ExportId, ExportRequest, ExportState, LogExporter};
use rivium_http::extract::{Json, Path};
use rivium_http::{ApiError, ApiResponse, ApiResult, NoEnvelope, file_response};
use serde::Deserialize;
use serde_json::{Value, json};

/// The kinds of logs the clients know, and the log files they are: the main file is named after
/// the service; `access` is a category file that the configuration may add.
const KINDS: [(&str, &str); 2] = [("service", "http-api"), ("access", "access")];

/// The routes of the binding.
pub fn router(exporter: LogExporter) -> Router {
    Router::new()
        .route("/api/logs/kinds", get(kinds_of_logs))
        .route("/api/logs/exports", post(start))
        .route("/api/logs/exports/{id}", get(progress).delete(cancel))
        .route("/api/logs/exports/{id}/archive", get(archive))
        .with_state(exporter)
}

async fn kinds_of_logs(State(exporter): State<LogExporter>) -> ApiResult<Vec<Value>> {
    let sinks = exporter.sinks();
    let there = KINDS
        .iter()
        .filter(|(_, sink)| sinks.iter().any(|name| name == sink));
    Ok(ApiResponse::Data(
        there.map(|(kind, _)| json!({ "kind": kind })).collect(),
    ))
}

#[derive(Deserialize)]
struct Start {
    from: Date,
    to: Date,
    #[serde(default)]
    kinds: Vec<String>,
}

async fn start(
    State(exporter): State<LogExporter>,
    Json(body): Json<Start>,
) -> Result<Response, ApiError> {
    let sink = |kind: &String| {
        KINDS
            .iter()
            .find(|(known, _)| known == kind)
            .map(|(_, sink)| *sink)
    };
    let unknown: Vec<&String> = body
        .kinds
        .iter()
        .filter(|kind| sink(kind).is_none())
        .collect();
    if !unknown.is_empty() {
        // The clients' own answer instead of the envelope: what they asked for that does not
        // exist, and what does.
        let known: Vec<&str> = KINDS.iter().map(|(kind, _)| *kind).collect();
        let answer = ApiResponse::Data(json!({ "unknown": unknown, "kinds": known }));
        return Ok((StatusCode::BAD_REQUEST, NoEnvelope, answer).into_response());
    }
    let request = ExportRequest {
        from: body.from,
        to: body.to,
        sinks: body
            .kinds
            .iter()
            .filter_map(sink)
            .map(String::from)
            .collect(),
    };
    let id = exporter.start(request)?;
    Ok(ApiResponse::Created(json!({ "id": id })).into_response())
}

async fn progress(
    State(exporter): State<LogExporter>,
    Path(id): Path<ExportId>,
) -> ApiResult<Value> {
    let progress = exporter.progress(id)?;
    let state = match progress.state {
        ExportState::Running => "running",
        ExportState::Done => "done",
        ExportState::Failed => "failed",
        _ => "cancelled",
    };
    let mut answer = json!({ "percent": progress.percent, "state": state });
    if let (ExportState::Failed, Err(error)) = (progress.state, exporter.archive(id)) {
        answer["error"] = json!(error.to_string());
    }
    Ok(ApiResponse::Data(answer))
}

async fn cancel(State(exporter): State<LogExporter>, Path(id): Path<ExportId>) -> ApiResult {
    exporter.cancel(id);
    Ok(ApiResponse::Ok)
}

async fn archive(
    State(exporter): State<LogExporter>,
    Path(id): Path<ExportId>,
    request: Request,
) -> Response {
    match exporter.archive(id) {
        Ok(archive) => file_response(request, &archive.path, &archive.file_name).await,
        Err(error) => ApiError(error).into_response(),
    }
}
