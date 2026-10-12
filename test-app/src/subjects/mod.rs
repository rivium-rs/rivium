//! Test subject modules.
//!
//! Each module implements a test scenario that can be verified by
//! tests-integration. The modules export a `main()` function that
//! serves as the entry point for the corresponding [[bin]] target.

pub mod host;
pub mod host_deadline;
pub mod host_export;
pub mod host_fault;
