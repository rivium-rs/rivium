//! The error model: construction, display, the cause chain, log fields and the extension traits.

use std::collections::BTreeMap;
use std::error::Error as _;
use std::fmt;
use std::sync::{Arc, Mutex};

use rivium_error::prelude::*;
use rivium_error::{Fields, kinds};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const DEVICE_MISSING: ErrorType =
    &ErrorKind::new("DeviceMissing", Class::NotFound).titled("Device missing");
const STORE_BROKEN: ErrorType = &ErrorKind::new("StoreBroken", Class::Unavailable);

/// A foreign error with a foreign source, as libraries build them.
#[derive(Debug)]
struct Outer(std::io::Error);

impl fmt::Display for Outer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("outer failure")
    }
}

impl std::error::Error for Outer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A foreign error that keeps an error of this crate, boxed, as its source.
#[derive(Debug)]
struct Wrapper(BError);

impl fmt::Display for Wrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wrapper")
    }
}

impl std::error::Error for Wrapper {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn hops(error: &Error) -> Vec<String> {
    (error.chain())
        .map(|hop| match hop.downcast_ref::<Error>() {
            Some(typed) => typed.etype().name().to_string(),
            None => format!("foreign: {hop}"),
        })
        .collect()
}

#[test]
fn the_chain_follows_typed_and_foreign_causes() {
    let middle = Error::because(
        STORE_BROKEN,
        "write failed",
        Outer(std::io::Error::other("disk full")),
    );
    let outer = Error::because(DEVICE_MISSING, "while saving", middle);
    let expected = [
        "DeviceMissing",
        "StoreBroken",
        "foreign: outer failure",
        "foreign: disk full",
    ];
    assert_eq!(hops(&outer), expected);
    assert_eq!(outer.root_cause().to_string(), "disk full");
    let source = outer
        .source()
        .and_then(|source| source.downcast_ref::<Error>());
    assert_eq!(source.map(Error::etype), Some(STORE_BROKEN));

    // A boxed error of this crate, kept by a foreign error or passed as a cause, is one hop.
    let wrapped = Error::because(DEVICE_MISSING, "loading", Wrapper(Error::new(STORE_BROKEN)));
    assert_eq!(
        hops(&wrapped),
        ["DeviceMissing", "foreign: wrapper", "StoreBroken"]
    );
    let boxed: Box<dyn std::error::Error + Send + Sync> = Error::new(STORE_BROKEN).into();
    let direct = Error::because(DEVICE_MISSING, "loading", boxed);
    assert_eq!(hops(&direct), ["DeviceMissing", "StoreBroken"]);
    assert_eq!(
        Error::new(STORE_BROKEN).root_cause().to_string(),
        "StoreBroken"
    );
}

#[test]
fn display_shows_this_hop_and_alternate_shows_the_chain() {
    let io = std::io::Error::other("disk full");
    let error = Error::because(STORE_BROKEN, "write failed", io).more_context("saving device 7");
    assert_eq!(error.to_string(), "saving device 7");
    assert_eq!(
        format!("{error:#}"),
        "saving device 7: write failed: disk full"
    );
    // Without context, a hop shows its kind's name; an empty context counts as none.
    let bare = Error::because(DEVICE_MISSING, "", Error::new(STORE_BROKEN));
    assert_eq!(
        (bare.to_string(), bare.context()),
        ("DeviceMissing".to_string(), None)
    );
    assert_eq!(format!("{bare:#}"), "DeviceMissing: StoreBroken");
}

#[test]
fn fields_come_from_the_members_not_the_display_text() {
    let inner = Error::explain(STORE_BROKEN, "store is gone").into_up();
    let mut outer = Error::because(DEVICE_MISSING, "while loading", inner);
    outer.set_retry(true);
    let fields = outer.fields();
    assert_eq!(
        (fields.etype, fields.class, fields.source, fields.retry),
        ("DeviceMissing", "NotFound", "unset", true)
    );
    assert_eq!(fields.context, "while loading: store is gone");
    assert_eq!(fields.chain, "DeviceMissing,StoreBroken");
    assert_eq!(fields.cause, "");

    let foreign = Error::because(
        STORE_BROKEN,
        "write failed",
        Outer(std::io::Error::other("x")),
    );
    let fields = foreign.fields();
    assert_eq!(
        (fields.chain.as_str(), fields.cause.as_str()),
        ("StoreBroken,external,external", "outer failure")
    );
    assert_eq!(Error::new(STORE_BROKEN).fields().context, "");
}

#[test]
fn source_and_retry_follow_the_rules_for_wrapping() {
    let mut inner = Error::explain(STORE_BROKEN, "store is gone").into_down();
    inner.set_retry(true);
    // `more_context` keeps the kind, the responsible party and the retry hint.
    let more = inner.more_context("while loading");
    assert_eq!(
        (more.etype(), *more.esource(), more.retry()),
        (STORE_BROKEN, ErrorSource::Downstream, true)
    );
    assert_eq!(more.fields().chain, "StoreBroken,StoreBroken");
    // `because` starts a new error: the party is unset again, the retry hint is inherited.
    let because = Error::because(DEVICE_MISSING, "while saving", more);
    assert_eq!(
        (*because.esource(), because.retry()),
        (ErrorSource::Unset, true)
    );
    // A foreign cause has no retry hint.
    let foreign = Error::because(DEVICE_MISSING, "x", std::io::Error::other("y"));
    assert!(!foreign.retry());
    let parties = [
        Error::new(STORE_BROKEN).into_up().esource().as_str(),
        Error::new(STORE_BROKEN).into_down().esource().as_str(),
        Error::new(STORE_BROKEN).into_in().esource().as_str(),
    ];
    assert_eq!(parties, ["upstream", "downstream", "internal"]);
}

#[test]
fn the_extension_traits_wrap_results_and_options() {
    let failed: std::result::Result<(), std::io::Error> = Err(std::io::Error::other("refused"));
    let error = failed.or_err(STORE_BROKEN, "connecting").unwrap_err();
    assert_eq!(format!("{error:#}"), "connecting: refused");

    let called = std::cell::Cell::new(false);
    let ok: std::result::Result<u8, std::io::Error> = Ok(1);
    let value = ok.or_err_with(STORE_BROKEN, || {
        called.set(true);
        "never built"
    });
    assert_eq!((value.unwrap(), called.get()), (1, false));
    let failed: std::result::Result<(), std::io::Error> = Err(std::io::Error::other("refused"));
    let error = failed
        .or_err_with(STORE_BROKEN, || format!("connecting to {}", 7))
        .unwrap_err();
    assert_eq!(error.context(), Some("connecting to 7"));

    let not_an_error: std::result::Result<(), u32> = Err(42);
    let error = not_an_error
        .explain_err(kinds::INVALID_INPUT, |code| format!("code {code}"))
        .unwrap_err();
    assert_eq!(
        (format!("{error:#}"), error.etype()),
        ("code 42".to_string(), kinds::INVALID_INPUT)
    );

    let failed: std::result::Result<(), std::io::Error> = Err(std::io::Error::other("boom"));
    let error = failed.or_fail().unwrap_err();
    assert_eq!(
        (error.etype(), format!("{error:#}")),
        (kinds::INTERNAL, "Internal: boom".to_string())
    );

    let missing: Option<u8> = None;
    let error = missing.or_err(DEVICE_MISSING, "no device 7").unwrap_err();
    assert_eq!(
        (error.context(), error.source().is_none()),
        (Some("no device 7"), true)
    );
    assert_eq!(Some(3).or_err_with(DEVICE_MISSING, || "unused").unwrap(), 3);

    let error = Error::e_explain::<()>(STORE_BROKEN, "store is gone")
        .err_context(|| "while loading")
        .unwrap_err();
    assert_eq!(format!("{error:#}"), "while loading: store is gone");
    let error =
        Error::e_because::<()>(STORE_BROKEN, "write", std::io::Error::other("full")).unwrap_err();
    assert_eq!(format!("{error:#}"), "write: full");
}

#[test]
fn kinds_and_classes_have_stable_names() {
    assert_eq!(
        (
            DEVICE_MISSING.name(),
            DEVICE_MISSING.class(),
            DEVICE_MISSING.title()
        ),
        ("DeviceMissing", Class::NotFound, Some("Device missing"))
    );
    assert_eq!(
        (STORE_BROKEN.title(), STORE_BROKEN.to_string()),
        (None, "StoreBroken".to_string())
    );
    let common = [
        (kinds::INTERNAL, "Internal"),
        (kinds::INVALID_INPUT, "InvalidInput"),
        (kinds::INVALID_BODY, "InvalidBody"),
        (kinds::NOT_FOUND, "NotFound"),
        (kinds::CONFLICT, "Conflict"),
        (kinds::UNAVAILABLE, "Unavailable"),
        (kinds::TIMEOUT, "Timeout"),
    ];
    for (kind, name) in common {
        // The common kinds are named after their class.
        assert_eq!((kind.name(), kind.class().as_str()), (name, name));
    }
    let classes = [
        Class::InvalidInput,
        Class::InvalidBody,
        Class::Unauthenticated,
        Class::Forbidden,
        Class::NotFound,
        Class::Conflict,
        Class::TooManyRequests,
        Class::Unavailable,
        Class::Timeout,
        Class::Internal,
    ];
    let names: Vec<String> = classes.iter().map(ToString::to_string).collect();
    assert_eq!(
        names,
        [
            "InvalidInput",
            "InvalidBody",
            "Unauthenticated",
            "Forbidden",
            "NotFound",
            "Conflict",
            "TooManyRequests",
            "Unavailable",
            "Timeout",
            "Internal"
        ]
    );
    assert_eq!(Error::new(kinds::TIMEOUT).class(), Class::Timeout);
}

/// One captured event: level, target and every field as text.
#[derive(Debug)]
struct Captured {
    level: Level,
    target: String,
    fields: BTreeMap<String, String>,
}

/// A subscriber that keeps every event.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Captured>>>);

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        struct Fields(BTreeMap<String, String>);
        impl Visit for Fields {
            fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
                self.0
                    .insert(field.name().to_string(), format!("{value:?}"));
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                self.0.insert(field.name().to_string(), value.to_string());
            }
        }
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        let metadata = event.metadata();
        self.0.lock().unwrap().push(Captured {
            level: *metadata.level(),
            target: metadata.target().to_string(),
            fields: fields.0,
        });
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn captured(f: impl FnOnce()) -> Vec<Captured> {
    let capture = Capture::default();
    tracing::subscriber::with_default(capture.clone(), f);
    std::mem::take(&mut *capture.0.lock().unwrap())
}

#[test]
fn log_error_writes_every_field_then_the_callers() {
    let io = std::io::Error::other("disk full");
    let error = Error::because(STORE_BROKEN, "write failed", io).more_context("saving");
    let events = captured(|| {
        log_error!(
            Level::WARN,
            &error,
            service.name = "poller",
            "service failed"
        )
    });
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.level, Level::WARN);
    let field = |name: &str| event.fields.get(name).map(String::as_str);
    let Fields {
        etype,
        class,
        source,
        retry,
        context,
        chain,
        cause,
        ..
    } = error.fields();
    assert_eq!(field("error.type"), Some(etype));
    assert_eq!(field("error.class"), Some(class));
    assert_eq!(field("error.source"), Some(source));
    assert_eq!(field("error.retry"), Some(retry.to_string().as_str()));
    assert_eq!(field("error.context"), Some(context.as_str()));
    assert_eq!(field("error.chain"), Some(chain.as_str()));
    assert_eq!(field("error.cause"), Some(cause.as_str()));
    assert_eq!(
        (field("service.name"), field("message")),
        (Some("poller"), Some("service failed"))
    );
}

#[test]
fn log_at_level_logs_at_the_level_given_at_run_time() {
    let levels = [
        Level::ERROR,
        Level::WARN,
        Level::INFO,
        Level::DEBUG,
        Level::TRACE,
    ];
    let events = captured(|| {
        for level in levels {
            log_at_level!(level, n = 1, "plain");
            log_at_level!(target: "access", level, "targeted");
        }
    });
    let seen: Vec<(Level, &str)> = events
        .iter()
        .map(|e| (e.level, e.target.as_str()))
        .collect();
    let expected: Vec<(Level, &str)> = levels
        .iter()
        .flat_map(|level| [(*level, "fields"), (*level, "access")])
        .collect();
    assert_eq!(seen, expected);
}
