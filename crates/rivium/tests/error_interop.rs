use rivium::{
    Error, ResultExt,
    error::{Class, ErrorKind, Retry, Side, kinds},
};
use std::{error::Error as StdError, fmt, time::Duration};

#[derive(Debug)]
struct Foreign(std::io::Error);

impl fmt::Display for Foreign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("foreign adapter failed")
    }
}

impl StdError for Foreign {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.0)
    }
}

#[test]
fn context_accepts_foreign_errors_and_retains_their_chain() {
    let result: Result<(), Foreign> = Err(Foreign(std::io::Error::other("disk unavailable")));
    let error = result.context("write snapshot").unwrap_err();
    assert_eq!(error.class(), Class::Internal);
    assert_eq!(error.chain().count(), 3);
    assert_eq!(
        error.chain().last().unwrap().to_string(),
        "disk unavailable"
    );
}

#[test]
fn result_context_preserves_typed_classification_retry_and_disclosure() {
    let result: rivium::Result<()> = Err(Error::new(kinds::CONFLICT, "diagnostic")
        .with_side(Side::Upstream)
        .with_retry(Retry::After(Duration::from_secs(2)))
        .with_public_message("safe summary"));
    let error = result
        .with_context(|| "operation failed".into())
        .unwrap_err();
    assert_eq!(error.kind(), kinds::CONFLICT);
    assert_eq!(error.side(), Side::Upstream);
    assert_eq!(error.retry(), Retry::After(Duration::from_secs(2)));
    assert_eq!(error.public_message(), Some("safe summary"));
}

#[test]
fn display_separates_the_current_context_from_full_diagnostics() {
    let error = Error::internal("writing")
        .with_source(std::io::Error::other("disk full"))
        .context("saving");
    assert_eq!(error.to_string(), "saving");
    assert_eq!(format!("{error:#}"), "saving: writing: disk full");
}

#[test]
fn successful_results_do_not_evaluate_lazy_context_and_custom_kinds_stay_stable() {
    let ok: Result<u8, Foreign> = Ok(7);
    assert_eq!(
        ok.with_context(|| panic!("success must not evaluate error context"))
            .unwrap(),
        7
    );
    static CUSTOM: ErrorKind =
        ErrorKind::new("DeviceMissing", Class::NotFound).titled("Device missing");
    let error = Error::new(&CUSTOM, "private");
    assert_eq!(error.kind().name, "DeviceMissing");
    assert_eq!(error.kind().title, Some("Device missing"));
    assert_eq!(error.class(), Class::NotFound);
}

#[test]
fn builtin_kind_names_are_stable_and_independent_of_diagnostics() {
    for (kind, name, class) in [
        (kinds::INVALID_INPUT, "InvalidInput", Class::InvalidInput),
        (kinds::INVALID_BODY, "InvalidBody", Class::InvalidBody),
        (
            kinds::PAYLOAD_TOO_LARGE,
            "PayloadTooLarge",
            Class::PayloadTooLarge,
        ),
        (
            kinds::UNAUTHENTICATED,
            "Unauthenticated",
            Class::Unauthenticated,
        ),
        (kinds::FORBIDDEN, "Forbidden", Class::Forbidden),
        (kinds::NOT_FOUND, "NotFound", Class::NotFound),
        (kinds::CONFLICT, "Conflict", Class::Conflict),
        (
            kinds::TOO_MANY_REQUESTS,
            "TooManyRequests",
            Class::TooManyRequests,
        ),
        (kinds::UNAVAILABLE, "Unavailable", Class::Unavailable),
        (kinds::TIMEOUT, "Timeout", Class::Timeout),
        (kinds::INTERNAL, "Internal", Class::Internal),
    ] {
        let error = Error::new(kind, "unrelated diagnostic").context("wrapper");
        assert_eq!(error.kind().name(), name);
        assert_eq!(error.class(), class);
    }
}
