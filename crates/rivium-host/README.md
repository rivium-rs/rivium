# rivium-host

Application composition and ownership for process/embedded execution. App supplies identity,
typed configuration, validation, pure hot-field selection and service construction.

Features are `process`, `embedded`, optional `log-export`, and `full`. Empty default features
still include configuration/logging and Tokio runtime dependencies; they only omit host entry points.

The process host provides run/check-config/default-config/version/help, root/config/overrides,
SIGINT/SIGTERM handling and stable exit codes. SIGQUIT retains its OS behavior. Inspection commands
have no configuration/log creation side effects. Embedded hosts require an explicit root and do not
install signals, read process environment or replace global tracing/panic hooks.

One host session owns a runtime; each restart creates a new generation with fresh configuration,
services and logger after old resources release. HostReport keeps primary and cleanup failures
separate. CleanupReport records closed admission and actual remaining service/reload/log owners.
Host::drain observes its captured generation; it does not request stop or follow a replacement.

Reload requests are serialized and bounded. Candidate/effective validation failure preserves the
old snapshot. Context::control returns HostControl: stop/restart synchronously close this
generation's reload admission before forwarding control, including during service construction.
Successful publication does not prove that external resources switched. Host log and
lifecycle settings remain restart-only. stop(within) uses one caller budget, and already-finished
hosts return NotRunning while their last report remains readable.

Requires Rust 1.92. See workspace examples for complete applications.
Licensed under MIT or Apache-2.0.
