//! Test application library for Rivium integration tests.
//!
//! This library contains the business logic for test programs that verify
//! Rivium's process-level behavior. Each [[bin]] target is a thin wrapper
//! around a subject module here.
//!
//! The subjects are designed to be tested via tests-integration, which spawns
//! them as subprocesses and verifies their behavior.

pub mod subjects;
