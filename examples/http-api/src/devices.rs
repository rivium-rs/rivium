//! A device registry, kept in memory: handlers that answer with the status envelope.
//!
//! | Request | Answer |
//! | --- | --- |
//! | `GET /api/devices?kind=…` | 200, the devices, of that kind if one is given |
//! | `GET /api/devices/{id}` | 200, the device; 404 when there is none |
//! | `POST /api/devices` with `{"name":…,"kind":…}` | 201, the device with its id; 409 when the name is taken, 406 when the body is not a device |
//! | `DELETE /api/devices/{id}` | 200 with the success envelope; 404 when there is none |

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use rivium::error::{Class, Error, ErrorKind, ErrorType};
use rivium_http::extract::{Json, Path, Query};
use rivium_http::{ApiResponse, ApiResult};
use serde::{Deserialize, Serialize};

/// No device has the id.
const UNKNOWN_DEVICE: ErrorType = &ErrorKind::new("UnknownDevice", Class::NotFound);
/// Another device has the name.
const NAME_TAKEN: ErrorType = &ErrorKind::new("DeviceNameTaken", Class::Conflict);
/// A device's field breaks a rule.
const INVALID_DEVICE: ErrorType = &ErrorKind::new("InvalidDevice", Class::InvalidBody);

/// A registered device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Its number, given when it is registered.
    pub id: u32,
    /// A unique name.
    pub name: String,
    /// What it is, such as `printer`.
    pub kind: String,
}

/// A device to register.
#[derive(Deserialize)]
struct NewDevice {
    name: String,
    kind: String,
}

/// The devices of one kind.
#[derive(Deserialize)]
struct Filter {
    kind: Option<String>,
}

type Devices = Arc<Mutex<BTreeMap<u32, Device>>>;

/// The routes of the registry.
pub fn router() -> Router {
    Router::new()
        .route("/api/devices", get(list).post(add))
        .route("/api/devices/{id}", get(one).delete(remove))
        .with_state(Devices::default())
}

fn lock(devices: &Devices) -> std::sync::MutexGuard<'_, BTreeMap<u32, Device>> {
    devices.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn list(
    State(devices): State<Devices>,
    Query(filter): Query<Filter>,
) -> ApiResult<Vec<Device>> {
    let devices = lock(&devices);
    let wanted = |device: &&Device| filter.kind.as_ref().is_none_or(|kind| *kind == device.kind);
    Ok(ApiResponse::Data(
        devices.values().filter(wanted).cloned().collect(),
    ))
}

async fn one(State(devices): State<Devices>, Path(id): Path<u32>) -> ApiResult<Device> {
    match lock(&devices).get(&id) {
        Some(device) => Ok(ApiResponse::Data(device.clone())),
        None => Err(Error::explain(UNKNOWN_DEVICE, format!("no device {id}")).into()),
    }
}

async fn add(State(devices): State<Devices>, Json(new): Json<NewDevice>) -> ApiResult<Device> {
    if new.name.trim().is_empty() {
        return Err(Error::explain(INVALID_DEVICE, "name: must not be empty").into());
    }
    let mut devices = lock(&devices);
    if devices.values().any(|device| device.name == new.name) {
        let why = format!("a device is named {:?} already", new.name);
        return Err(Error::explain(NAME_TAKEN, why).into());
    }
    let id = devices.keys().next_back().map_or(1, |last| last + 1);
    let device = Device {
        id,
        name: new.name,
        kind: new.kind,
    };
    devices.insert(id, device.clone());
    Ok(ApiResponse::Created(device))
}

async fn remove(State(devices): State<Devices>, Path(id): Path<u32>) -> ApiResult {
    match lock(&devices).remove(&id) {
        Some(_) => Ok(ApiResponse::Ok),
        None => Err(Error::explain(UNKNOWN_DEVICE, format!("no device {id}")).into()),
    }
}
