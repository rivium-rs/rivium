//! The contract layer on a router, driven with `oneshot` and without a server: every check of
//! the contract, and the observers, which only the contract layer calls.

mod common;

use std::sync::{Arc, Mutex};

use rivium_http::{ContractSettings, HttpObserver, ResponseInfo, contract};
use serde_json::Value;

/// A response as an observer saw it: method, route, status, request id, error type.
type Seen = (String, Option<String>, u16, String, Option<String>);

/// Records what it sees, and logs that it saw it, so that the order of calls shows in the log.
struct Observer {
    name: &'static str,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl HttpObserver for Observer {
    fn on_response(&self, info: &ResponseInfo<'_>) {
        let error = info.error.map(|error| error.etype().name().to_string());
        let route = info.route.map(String::from);
        let seen = (
            info.method.to_string(),
            route,
            info.status.as_u16(),
            info.request_id.to_string(),
            error,
        );
        self.seen.lock().unwrap().push(seen);
        tracing::info!(observer = self.name, id = info.request_id, "observed");
    }
}

#[tokio::test(start_paused = true)]
async fn the_contract_layer_keeps_the_contract() {
    let logs = rivium_test::capture_logs();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observers: Vec<Arc<dyn HttpObserver>> = ["first", "second"]
        .map(|name| -> Arc<dyn HttpObserver> {
            let seen = Arc::clone(&seen);
            Arc::new(Observer { name, seen })
        })
        .into();
    let keep = |routes, expose| {
        let mut settings = ContractSettings::default();
        settings.request_timeout = common::TIMEOUT;
        settings.body_limit = common::BODY_LIMIT;
        settings.expose_internal_detail = expose;
        contract(routes, &settings, observers.clone())
    };
    common::check(keep, &logs).await;

    // Each response is seen once by each observer, in the order they were given, after its
    // access log line.
    let app = keep(common::routes(), false);
    let reply = common::get_(&app, "/error/Conflict").await;
    let id = reply.request_id();
    let order: Vec<String> = (logs.events().into_iter())
        .filter(|event| {
            let about = |key| event.get(key).and_then(Value::as_str) == Some(id);
            about("request_id") || about("id")
        })
        .map(
            |event| match event.get("observer").and_then(Value::as_str) {
                Some(observer) => observer.to_string(),
                None => event["message"].as_str().unwrap().to_string(),
            },
        )
        .collect();
    assert_eq!(order, ["request", "first", "second"]);
    let seen = seen.lock().unwrap();
    let mine: Vec<&Seen> = seen.iter().filter(|seen| seen.3 == id).collect();
    let expected = (
        "GET".to_string(),
        Some("/error/{class}".to_string()),
        409,
        id.to_string(),
        Some("Failing".to_string()),
    );
    assert_eq!(mine, [&expected, &expected]);
    let ok = seen
        .iter()
        .find(|seen| seen.1.as_deref() == Some("/ok"))
        .unwrap();
    assert_eq!((ok.0.as_str(), ok.2, ok.4.as_deref()), ("GET", 200, None));
    let unrouted = seen.iter().find(|seen| seen.2 == 404 && seen.1.is_none());
    assert!(unrouted.is_some(), "an unknown route has no route");
    let own = seen
        .iter()
        .find(|seen| seen.1.as_deref() == Some("/own"))
        .unwrap();
    assert_eq!(
        (own.2, own.4.as_deref()),
        (409, None),
        "NoEnvelope carries no error"
    );
}
