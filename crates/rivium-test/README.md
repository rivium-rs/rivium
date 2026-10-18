# rivium-test

Small public-contract test tools. Add this package only as a dev-dependency.

TempDir owns and removes a unique temporary directory. Logs captures small test output through
a scoped Dispatch. idle creates a cooperative service, and until_ready waits under a finite
budget. Captured logs are intentionally in-memory test data, not a bounded production sink.

`command` launches a target executable using `CARGO_TARGET_<TRIPLE>_RUNNER` when configured.
Its build script records Cargo's compilation target, so a different runtime
`CARGO_BUILD_TARGET` cannot select the wrong runner. Runner arguments are whitespace-separated;
Windows children launch directly when their parent is already running inside Wine.

Complete fault applications, processes, JNI loading and model-checking suites stay in the
repository's private validation packages. This crate does not create a new runtime, test macro
framework or application host abstraction.

Requires Rust 1.92. Licensed under MIT or Apache-2.0.
