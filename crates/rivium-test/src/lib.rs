//! Test support for services built on Rivium.
//!
//! This crate will provide scripted services, log capture, process-level test tools and the
//! lifecycle contract suites. Use it as a dev-dependency only. It is pre-release: so far it
//! provides [`deps`], the dependency closure check.
#![forbid(unsafe_code)]

pub mod deps;
