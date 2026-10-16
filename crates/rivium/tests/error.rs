use rivium::{
    Error,
    error::{Class, Retry, Side, kinds},
};

#[test]
fn context_preserves_classification_and_explicit_disclosure() {
    let error = Error::new(kinds::INVALID_INPUT, "secret validation detail")
        .with_public_message("invalid station")
        .with_retry(Retry::Never)
        .context("secret request id");
    assert_eq!(error.class(), Class::InvalidInput);
    assert_eq!(error.side(), Side::Caller);
    assert_eq!(error.public_message(), Some("invalid station"));
    assert!(error.to_string().contains("secret request id"));
    assert_eq!(Error::internal("password").public_message(), None);
}

#[test]
fn disclosure_is_bounded_at_a_utf8_boundary() {
    let error = Error::internal("private").with_public_message("界".repeat(400));
    assert_eq!(error.public_message().unwrap().len(), 1023);
}

#[test]
fn context_retains_diagnostic_sources_without_changing_kind() {
    let error = Error::invalid("decode failed")
        .with_source(std::io::Error::other("source"))
        .context("load config");
    assert_eq!(error.kind(), kinds::INVALID_INPUT);
    assert_eq!(error.chain().count(), 3);
    assert!(error.chain().last().unwrap().to_string().contains("source"));
}
