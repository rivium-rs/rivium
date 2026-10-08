//! The error type shared by Rivium services.
//!
//! This crate will provide one error type with a kind, a transport-independent class, the
//! responsible party, a retry hint, context and a cause chain, together with the structured log
//! fields that every service emits for it. It is pre-release: nothing is implemented yet.
#![forbid(unsafe_code)]
