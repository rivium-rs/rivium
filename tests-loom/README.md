# Scope concurrency models

Run `RUSTFLAGS="--cfg rivium_scope_model" cargo test --manifest-path tests-loom/Cargo.toml`.

This isolated package compiles the actual rewritten scope source. Only synchronization imports
and the upstream cancellation flag are substituted. Six models cover admission/closure,
concurrent reservation release, cancellation, and release ordering for unpolled/completed
futures. Tokio's Notify algorithm and asynchronous scheduling are outside these models and
are exercised by the core integration tests.

The old generation TaskTracker model was replaced with concurrent batch accounting because
the rebuilt supervisor observes root and service scope counts directly; no generation
TaskTracker exists in the new production implementation.
