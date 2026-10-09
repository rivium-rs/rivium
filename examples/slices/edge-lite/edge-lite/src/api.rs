//! The HTTP API, with the status envelope. Its log export routes are this service's binding of
//! Rivium's log export: their paths and field names are the service's own.
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET /api/units` | `[{"id":0,"state":"running","polls":12,"failures":0,"last":203},…]` |
//! | `GET /api/units/{id}` | one unit; 404 when there is none |
//! | `GET /api/config` | the configuration of the run, as JSON |
//! | `PUT /api/config` with a TOML file | checks it as the next start would load it (406 with every problem), writes it with a backup and asks for a restart, which loads it |
//! | `GET /api/logs` | `{"files":["edge-lite","units"]}`: the log files to export |
//! | `POST /api/logs/export` with `{"from":"2026-10-09","to":"2026-10-09","files":["units"]}` | 201 `{"id":3}`; every file when `files` is left out |
//! | `GET /api/logs/export/{id}` | `{"percent":100,"state":"done"}` |
//! | `GET /api/logs/export/{id}/file` | the zip archive |

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rivium::AppContext;
use rivium::error::{Error, OrErr, kinds};
use rivium::log::{Date, ExportId, ExportRequest, ExportState, LogExporter};
use rivium_http::extract::{Json, Path};
use rivium_http::{ApiError, ApiResponse, ApiResult, file_response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Config;
use crate::units::{Status, UnitState};

/// What the handlers share: the run's configuration, the units, and the run's context, which
/// checks a new configuration and asks for the restart that loads it.
#[derive(Clone)]
struct Api {
    config: Arc<Config>,
    status: Status,
    ctx: AppContext,
}

/// The routes.
pub fn router(config: Config, status: Status, ctx: AppContext) -> Router {
    let exports = Router::new()
        .route("/api/logs", get(files))
        .route("/api/logs/export", post(export))
        .route("/api/logs/export/{id}", get(progress))
        .route("/api/logs/export/{id}/file", get(download))
        .with_state(ctx.log_exporter().clone());
    let api = Api {
        config: Arc::new(config),
        status,
        ctx,
    };
    Router::new()
        .route("/api/units", get(units))
        .route("/api/units/{id}", get(unit))
        .route("/api/config", get(configuration).put(replace))
        .with_state(api)
        .merge(exports)
}

async fn units(State(api): State<Api>) -> ApiResult<Vec<UnitState>> {
    Ok(ApiResponse::Data(api.status.snapshot()))
}

async fn unit(State(api): State<Api>, Path(id): Path<usize>) -> ApiResult<UnitState> {
    let unit = api.status.snapshot().into_iter().find(|unit| unit.id == id);
    let unit = unit.ok_or_else(|| Error::explain(kinds::NOT_FOUND, format!("no unit {id}")))?;
    Ok(ApiResponse::Data(unit))
}

async fn configuration(State(api): State<Api>) -> ApiResult<Config> {
    Ok(ApiResponse::Data(Config::clone(&api.config)))
}

/// Check, write, restart: the restart loads the new file.
async fn replace(State(api): State<Api>, candidate: String) -> ApiResult {
    if let Err(problems) = api.ctx.check_config(&candidate) {
        return Err(Error::explain(kinds::INVALID_BODY, problems.to_string()).into());
    }
    let file = api.ctx.paths().config_file();
    rivium::fs::atomic_write(file, candidate.as_bytes(), true)
        .or_err_with(kinds::INTERNAL, || format!("writing {}", file.display()))?;
    tracing::info!(file = %file.display(), "the configuration was replaced: restarting");
    api.ctx
        .restarter()
        .request("the configuration was replaced");
    Ok(ApiResponse::Ok)
}

async fn files(State(exporter): State<LogExporter>) -> ApiResult<Value> {
    Ok(ApiResponse::Data(json!({ "files": exporter.sinks() })))
}

#[derive(Deserialize)]
struct Export {
    from: Date,
    to: Date,
    #[serde(default)]
    files: Vec<String>,
}

async fn export(State(exporter): State<LogExporter>, Json(body): Json<Export>) -> ApiResult<Value> {
    let (from, to, sinks) = (body.from, body.to, body.files);
    let id = exporter.start(ExportRequest { from, to, sinks })?;
    Ok(ApiResponse::Created(json!({ "id": id })))
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
    Ok(ApiResponse::Data(
        json!({ "percent": progress.percent, "state": state }),
    ))
}

async fn download(
    State(exporter): State<LogExporter>,
    Path(id): Path<ExportId>,
    request: Request,
) -> Response {
    match exporter.archive(id) {
        Ok(archive) => file_response(request, &archive.path, &archive.file_name).await,
        Err(error) => ApiError(error).into_response(),
    }
}
