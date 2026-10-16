#![cfg(feature = "config")]

use rivium_test::TempDir;
use rivium_util::config::{Loader, Source, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
struct Config {
    port: u16,
    label: String,
    maybe: Option<String>,
    names: Vec<String>,
    map: BTreeMap<String, u32>,
}

fn loader() -> Loader {
    Loader::new(&Config::default()).unwrap()
}

fn loaded(loader: Loader) -> rivium_util::config::Document {
    loader.load().unwrap()
}

#[test]
fn layers_keep_types_and_provenance() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("config.toml");
    std::fs::write(
        &file,
        "port=80\nlabel='file'\nmaybe='yes'\nnames=['a']\n[map]\nx=1\n",
    )
    .unwrap();
    let doc = loaded(
        loader()
            .file(&file)
            .environment("APP", [("APP_MAP__X".into(), "2".into())])
            .overrides(["port=8080".into(), "label=123".into()]),
    );
    let c: Config = doc.decode().unwrap();
    assert_eq!(c.port, 8080);
    assert_eq!(c.label, "123");
    assert_eq!(c.map["x"], 2);
    assert_eq!(c.maybe.as_deref(), Some("yes"));
    assert_eq!(doc.source("port"), Source::Override("port".into()));
    assert!(doc.listing().contains("env APP_MAP__X"));
}

#[test]
fn unknown_keys_and_type_errors_are_diagnostics() {
    let doc = loaded(loader().overrides(["typo=3".into(), "port=bad".into()]));
    assert!(doc.decode::<Config>().is_err());
    let doc = loaded(loader().overrides(["typo=3".into()]));
    assert!(
        doc.decode::<Config>()
            .unwrap_err()
            .to_string()
            .contains("unknown key")
    );
}

#[test]
fn optional_text_from_override() {
    let doc = loaded(loader().overrides(["maybe=123".into()]));
    assert_eq!(
        doc.decode::<Config>().unwrap().maybe.as_deref(),
        Some("123")
    );
}

#[test]
fn candidates_do_not_publish_until_valid() {
    let mut store = Store::new(1);
    let handle = store.handle();
    assert_eq!(*handle.current(), 1);
    assert_eq!(store.publish(2).unwrap(), 1);
    assert_eq!(*handle.current(), 2);
    assert_eq!(handle.generation(), 1);
    drop(handle);
    assert_eq!(store.publish(3).unwrap(), 2);
    assert_eq!(*store.handle().current(), 3);
}

#[tokio::test]
async fn config_handle_closes() {
    let store = Store::new(1);
    let mut handle = store.handle();
    drop(store);
    assert!(handle.changed().await.is_none());
}

#[tokio::test]
async fn snapshots_keep_matching_values_and_generations_after_publication_and_close() {
    struct NonClone(u32);

    let mut store = Store::new(NonClone(1));
    let mut handle = store.handle();
    let initial = handle.snapshot();
    let retained = initial.clone();
    assert!(std::sync::Arc::ptr_eq(&initial.value, &retained.value));

    assert_eq!(store.publish(NonClone(2)).unwrap(), 1);
    let updated = handle.snapshot();
    assert_eq!((retained.generation, retained.value.0), (0, 1));
    assert_eq!((updated.generation, updated.value.0), (1, 2));

    drop(store);
    // Reading a snapshot must not consume the final unseen watch update.
    assert_eq!(handle.changed().await.unwrap().0, 2);
    assert!(handle.changed().await.is_none());
    let closed = handle.snapshot();
    assert_eq!((closed.generation, closed.value.0), (1, 2));
    assert_eq!((retained.generation, retained.value.0), (0, 1));
}

#[test]
fn invalid_fields_are_collected_together() {
    let doc = loaded(loader().overrides(["port=bad".into(), "map={x='bad'}".into()]));
    let report = doc.decode::<Config>().unwrap_err().to_string();
    assert!(report.contains("port"), "{report}");
    assert!(report.contains("map.x"), "{report}");
}

#[test]
fn parent_override_replaces_earlier_child_text_and_provenance() {
    let defaults = Config {
        map: BTreeMap::from([("default".into(), 9)]),
        ..Default::default()
    };
    let doc = loaded(
        Loader::new(&defaults)
            .unwrap()
            .overrides(["map.x=1".into(), "map={x=2}".into()]),
    );
    assert_eq!(
        doc.decode::<Config>().unwrap().map,
        BTreeMap::from([("x".into(), 2)])
    );
    assert!(doc.listing().contains("map.x = 2 (--set map)"));
    assert_eq!(doc.source("map.x"), Source::Override("map".into()));
    assert_eq!(doc.source("map.default"), Source::Override("map".into()));
}

#[test]
fn parent_cli_override_discards_invalid_environment_child_without_touching_siblings() {
    let doc = loaded(
        loader()
            .environment("APP", [("APP_MAP__X".into(), "not-a-number".into())])
            .overrides(["label=123".into(), "map={x=2}".into()]),
    );
    let config: Config = doc.decode().unwrap();
    assert_eq!(config.map["x"], 2);
    assert_eq!(config.label, "123");
    assert_eq!(doc.source("map.x"), Source::Override("map".into()));
    assert_eq!(doc.source("label"), Source::Override("label".into()));
}

#[test]
fn a_later_child_override_keeps_the_replacement_tables_other_fields() {
    let doc =
        loaded(loader().overrides(["map.x=1".into(), "map={x=2,y=3}".into(), "map.y=4".into()]));
    assert_eq!(
        doc.decode::<Config>().unwrap().map,
        BTreeMap::from([("x".into(), 2), ("y".into(), 4)])
    );
    assert_eq!(doc.source("map.x"), Source::Override("map".into()));
    assert_eq!(doc.source("map.y"), Source::Override("map.y".into()));
}

#[test]
fn replacing_a_branch_does_not_clear_a_similarly_named_sibling() {
    let defaults = BTreeMap::from([
        ("map", BTreeMap::from([("x", 0)])),
        ("maple", BTreeMap::from([("x", 0)])),
    ]);
    let doc = loaded(Loader::new(&defaults).unwrap().overrides([
        "maple.x=9".into(),
        "map.x=1".into(),
        "map={x=2}".into(),
    ]));
    let values: BTreeMap<String, BTreeMap<String, u32>> = doc.decode().unwrap();
    assert_eq!(values["map"]["x"], 2);
    assert_eq!(values["maple"]["x"], 9);
    assert_eq!(doc.source("maple.x"), Source::Override("maple.x".into()));
}

#[test]
fn guarded_publication_releases_gate_before_waking_or_dropping_old_values() {
    use std::{
        future::Future,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Wake, Waker},
    };

    struct Value {
        number: u64,
        on_drop: Option<Box<dyn Fn() + Send + Sync>>,
    }
    impl Drop for Value {
        fn drop(&mut self) {
            if let Some(on_drop) = &self.on_drop {
                on_drop();
            }
        }
    }
    struct Observer {
        gate: Arc<Mutex<()>>,
        config: rivium_util::config::Handle<Value>,
        observed: AtomicBool,
    }
    impl Wake for Observer {
        fn wake(self: Arc<Self>) {
            assert!(
                self.gate.try_lock().is_ok(),
                "notification retained the gate"
            );
            let snapshot = self.config.snapshot();
            assert_eq!((snapshot.generation, snapshot.value.number), (1, 2));
            self.observed.store(true, Ordering::Release);
        }
    }

    let gate = Arc::new(Mutex::new(()));
    let released = Arc::new(AtomicBool::new(false));
    let (old_gate, old_released) = (gate.clone(), released.clone());
    let mut store = Store::new(Value {
        number: 1,
        on_drop: Some(Box::new(move || {
            assert!(
                old_gate.try_lock().is_ok(),
                "old value dropped inside the gate"
            );
            old_released.store(true, Ordering::Release);
        })),
    });
    let observer = Arc::new(Observer {
        gate: gate.clone(),
        config: store.handle(),
        observed: AtomicBool::new(false),
    });
    let mut reader = store.handle();
    let mut changed = Box::pin(reader.changed());
    let waker = Waker::from(observer.clone());
    assert!(
        changed
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert_eq!(
        store
            .publish_guarded(
                Value {
                    number: 2,
                    on_drop: None
                },
                gate.lock().unwrap()
            )
            .unwrap(),
        1
    );
    assert!(observer.observed.load(Ordering::Acquire));
    assert!(released.load(Ordering::Acquire));
    assert!(
        changed
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_ready()
    );
}

#[test]
fn document_limits_reject_bytes_before_parsing() {
    use rivium::error::kinds;
    use rivium_util::config::LoadLimits;
    let limits = LoadLimits {
        max_document_bytes: 6,
    };
    let error = loader()
        .limits(limits)
        .load_text(Some("label='oversized'"))
        .unwrap_err();
    assert_eq!(error.kind(), kinds::PAYLOAD_TOO_LARGE);
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("large.toml");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let error = loader().file(&path).load().unwrap_err();
    assert_eq!(error.kind(), kinds::PAYLOAD_TOO_LARGE);
    assert_eq!(error.0[0].source, Source::File(path));
    let doc = loader().limits(limits).load_text(Some("port=8")).unwrap();
    assert_eq!(doc.decode::<Config>().unwrap().port, 8);
}

#[test]
fn explicit_environment_decodes_options_maps_and_lists() {
    let doc = loaded(loader().environment(
        "APP",
        [
            ("APP_MAYBE".into(), "123".into()),
            ("APP_NAMES".into(), "['first', 'second']".into()),
            ("APP_MAP__ONE".into(), "42".into()),
            ("OTHER_PORT".into(), "99".into()),
        ],
    ));
    let config: Config = doc.decode().unwrap();
    assert_eq!(config.maybe.as_deref(), Some("123"));
    assert_eq!(config.names, ["first", "second"]);
    assert_eq!(config.map["one"], 42);
    assert_eq!(config.port, 0);
}

#[test]
fn section_diagnostics_keep_the_full_path_and_source() {
    let defaults = BTreeMap::from([("service", Config::default())]);
    let doc = loaded(
        Loader::new(&defaults)
            .unwrap()
            .overrides(["service.port=invalid".into()]),
    );
    let error = doc.section::<Config>("service").unwrap_err();
    assert_eq!(error.0[0].key, "service.port");
    assert_eq!(error.0[0].source, Source::Override("service.port".into()));
}

#[test]
fn snapshots_remain_consistent_during_concurrent_publication() {
    let mut store = Store::new(0_u64);
    let handle = store.handle();
    let worker = std::thread::spawn(move || {
        for _ in 0..10_000 {
            let snapshot = handle.snapshot();
            assert_eq!(*snapshot.value, snapshot.generation);
        }
    });
    for value in 1..=10_000 {
        assert_eq!(store.publish(value).unwrap(), value);
    }
    worker.join().unwrap();
}

#[test]
fn list_type_errors_keep_assignment_origin_and_other_field_diagnostics() {
    let doc = loaded(loader().overrides(["names=[1]".into(), "port=invalid".into()]));
    let report = doc.decode::<Config>().unwrap_err();
    let list = report
        .0
        .iter()
        .find(|problem| problem.key == "names[0]")
        .unwrap();
    assert_eq!(list.source, Source::Override("names".into()));
    assert!(report.0.iter().any(|problem| problem.key == "port"));
}

#[test]
fn an_assignment_cannot_smuggle_additional_toml_fields() {
    let doc = loaded(loader().overrides(["port=8\nextra=9".into()]));
    assert!(doc.decode::<Config>().is_err());
}

#[test]
fn typed_environment_supports_unsigned_values_beyond_tomls_signed_range() {
    #[derive(Serialize, Deserialize)]
    struct Large {
        limit: u64,
    }
    let document = loaded(
        Loader::new(&Large { limit: 0 })
            .unwrap()
            .environment("APP", [("APP_LIMIT".into(), "18446744073709551615".into())]),
    );
    assert_eq!(document.decode::<Large>().unwrap().limit, u64::MAX);
}

#[test]
fn compound_types_and_enums_keep_environment_provenance() {
    #[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "kebab-case")]
    enum Mode {
        #[default]
        Active,
        Passive,
    }
    #[derive(Default, Serialize, Deserialize)]
    struct Compound {
        maybe: Option<u16>,
        mode: Mode,
        targets: Vec<BTreeMap<String, String>>,
    }
    let doc = loaded(Loader::new(&Compound::default()).unwrap().environment(
        "APP",
        [
            ("APP_MAYBE".into(), "42".into()),
            ("APP_MODE".into(), "passive".into()),
            ("APP_TARGETS".into(), "[{host='example'}]".into()),
        ],
    ));
    let config = doc.decode::<Compound>().unwrap();
    assert_eq!(config.maybe, Some(42));
    assert_eq!(config.mode, Mode::Passive);
    assert_eq!(config.targets[0]["host"], "example");
    assert_eq!(doc.source("mode"), Source::Environment("APP_MODE".into()));
}

#[test]
fn diagnostics_are_unique_sourced_and_single_line() {
    let doc = loaded(
        loader()
            .environment("APP", [("APP_PORT".into(), "70000".into())])
            .overrides(["map={x='bad'}".into(), "unknown=1".into()]),
    );
    let report = doc.decode::<Config>().unwrap_err();
    for key in ["port", "map.x", "unknown"] {
        assert_eq!(
            report.0.iter().filter(|problem| problem.key == key).count(),
            1,
            "{key} must have exactly one diagnostic"
        );
    }
    let port = report
        .0
        .iter()
        .find(|problem| problem.key == "port")
        .unwrap();
    assert_eq!(port.source, Source::Environment("APP_PORT".into()));
    assert_eq!(report.to_string().lines().count(), report.0.len());
}

#[test]
fn whole_document_errors_retain_file_position_and_single_line_diagnostics() {
    let path = std::path::PathBuf::from("config.toml");
    let report = loader()
        .file(&path)
        .load_text(Some("label='valid'\nport = = 3\n"))
        .unwrap_err();
    assert_eq!(report.0[0].source, Source::File(path));
    assert!(report.to_string().contains("line 2"), "{report}");
    assert_eq!(report.to_string().lines().count(), 1);
}

#[test]
fn rust_log_fallback_is_below_prefixed_filter_and_explicit_assignment() {
    let defaults = BTreeMap::from([("log", BTreeMap::from([("filter", "info")]))]);
    let document = Loader::new(&defaults)
        .unwrap()
        .environment(
            "APP",
            [
                ("RUST_LOG".into(), "trace".into()),
                ("APP_LOG__FILTER".into(), "warn".into()),
            ],
        )
        .overrides(["log.filter=error".into()])
        .load_text(Some("[log]\nfilter='debug'\n"))
        .unwrap();
    assert_eq!(document.value["log"]["filter"].as_str(), Some("error"));
    assert!(
        document
            .listing()
            .contains("log.filter = \"error\" (--set log.filter)")
    );
}

#[test]
fn child_assignment_clears_replaced_scalar_ancestor_text() {
    let document = loaded(
        Loader::new(&BTreeMap::from([("port", 80_u16)]))
            .unwrap()
            .overrides(["port=123".into(), "port.child=456".into()]),
    );
    assert!(document.value["port"].is_table());
    assert!(document.decode::<BTreeMap<String, u16>>().is_err());
    assert_ne!(document.source("port"), Source::Override("port".into()));
}
