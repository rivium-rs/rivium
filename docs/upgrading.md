# Upgrading

Keep all Rivium packages on the same version. This page describes the 0.2 source rewrite
and the historical 0.1 maintenance records below.

## 0.2.0

Released rewrite.

This is a complete production-source rewrite on the 0.1.1 history. Old implementations
serve as behavior evidence; Rust compatibility shims are not included. Preserve your
application configuration/data before migrating, and test actual external protocols.

| 0.1 / earlier releases | 0.2 destination or behavior |
|---|---|
| rivium-error | rivium::error; default core has no dependencies |
| rivium::App/process/embedded | rivium-host with explicit process/embedded features |
| Configuration/logging/task helpers in the service foundation | independently selected rivium-util features |
| Supervisor::run -> Outcome | RunReport with separate outcome and drain |
| Implicit spawn failure conversion | match SubmitError, especially Closed during normal shutdown |
| Error context exposed in HTTP | explicit with_public_message opt-in; server/upstream diagnostics remain private |
| Broad error helper/macro facade | Error getters, source/chain, ResultExt, Side and Retry; no tracing dependency in default core |
| ResultExt over error types | accepts foreign standard errors; typed errors keep their classification and retry hint |
| Atomic write IO error only | AtomicWriteError includes stage and committed status |
| Manager arbitrary desired iterator / active-only capacity | finite Vec input, capacity includes all tracked keys; DesiredRejected preserves live tasks |
| HTTP Settings | ContractSettings with request capacity; ServerLimits separately bounds connections |
| Custom serve_listener with three arguments | explicit fourth ServerLimits argument |
| Context::control returning core Control | generation-bound HostControl; stop/restart also close reload admission synchronously |
| Host drain only service tasks | CleanupReport for the captured generation, including reload/log owners and errors |

The six public packages are rivium, rivium-util, rivium-host, rivium-http, rivium-jni and
rivium-test. The last belongs in dev-dependencies. Core, utility, host and HTTP feature
sets are additive; see their README files for the actual default closure.

Update error construction to Error::new(kind, diagnostic), context/with_source and explicit
Retry hints. Display prints the current diagnostic hop; alternate formatting (`{:#}`) prints
the cause chain. Retry hints never establish that a non-idempotent operation may be repeated.

Before disposing shared state, inspect drain/cleanup, not merely Phase::Stopped or a stop
request. Started blocking closures cannot be killed by Tokio. In-process restart reconstructs
configuration, services and logging only after the old owners release; the session runtime
is reused. Host.stop budgets the caller's total wait; zero requests stop and returns promptly.
An already-finished session returns NotRunning, preserving its previous report for inspection.

Reload defaults to restart-only configuration. Implement App::reload_config as a pure selection
of actual hot fields, validate the effective combination, and read one snapshot per logical
operation. Writing the file does not reload it. Capacity/resource settings that are consumed
only at startup must stay cold. The loader limits documents to 1 MiB by default.

File logging now drops an oversized event whole above 64 KiB; it does not publish truncated
JSON/text as a successful event. Observe instance counters and cleanup failures. Category
names and total budgets are validated. Historical log names/formats outside the documented
recognized archive set need an application migration; never assume every old log is imported.

HTTP handler timeouts end at response headers. Response/request body ownership continues until
release. Unknown-length body limits only apply to bytes actually read. NoEnvelope preserves
application protocol shapes while keeping admission and input limits. TLS remains an application
adapter. Existing clients with custom response/status conventions must keep their adapter.

The JNI five-method ABI and process result codes remain stable, but Java last-error strings are
safe summaries. Exact private diagnostic text and global panic-hook output are not contracts.
SIGQUIT keeps its operating-system disposition; the process host handles SIGINT/SIGTERM.

All historical test/benchmark/CI assets were saved before source removal. Passing counts are not
proof of equal coverage: migrated contract mappings and actual checks determine acceptance.
Platform evidence is version-specific; see [platforms](platforms.md).

## 0.1.1

This is a compatible maintenance update of the five library crates. Their production Rust
implementation, public APIs, configuration defaults, exit codes and Rust 1.92 MSRV are unchanged
from 0.1.0. Existing services do not need source or configuration migration. Keep all Rivium
crates on the same version and update the lockfile with `cargo update -p rivium --precise 0.1.1`;
dependency requirements such as `rivium = "0.1"` already allow 0.1.1.

The repository workflow changed even though the library contract did not:

- **The project template was removed from `main`.** A `cargo generate` invocation that uses this
  repository's default branch as its template source will no longer work. For the old scaffold,
  explicitly select the [`v0.1.0` tag](https://github.com/rivium-rs/rivium/tree/v0.1.0/template),
  or start a normal Cargo application using the [App example](https://docs.rs/rivium/latest/rivium/trait.App.html).
  The historical scaffold command is:

  ```bash
  cargo generate --git https://github.com/rivium-rs/rivium --tag v0.1.0 template
  ```

  Already-generated projects keep their own source; they do not need to be regenerated to
  update their Rivium dependencies. The generated release notes retain this repository-level
  breaking change; it is not a breaking change to the published library APIs.
- **The old repository examples and validation fixtures were removed from `main`.**
  The released examples remain at [`v0.1.0/examples`](https://github.com/rivium-rs/rivium/tree/v0.1.0/examples).
  Repository contributors should follow [development checks](development.md), not old paths
  under `examples/` or `fixtures/`. The new `test-app`, `test-jni`, `tests-integration` and
  `tests-build` packages are private validation tools, not new application dependencies.
- **Verification claims are version-specific.** Consult the [0.1.1 record](platforms.md#011)
  for the checked targets and omissions. A restored CI lane does not establish Android device,
  service-manager or hardware verification.

Build and test the updated service on its deployment targets before rolling it out. Retain
explicitly unverified platform claims; the 0.1.0 hardware/hosting record is not automatically
renewed by a patch version bump.

## 0.1.0

The first release: nothing to upgrade from.
