# rivium

Dependency-free structured errors by default. Enable `task` for TaskScope registration,
permanent admission closure and drain confirmation; enable `rt` for Service, Context,
Supervisor, readiness, health and staged shutdown. `full` selects `rt`.

Supervisor returns RunReport, separating the stop/failure cause from resource release.
Cancellation is cooperative; blocking work can survive an abort deadline. Context-owned
tasks have bounded admission and typed rejection. Dynamic task owners share a service scope
without relinquishing their own handles or recovery policy.

Error diagnostic context is private. Explicit public_message controls eligible adapter
disclosure; kind/class/side/retry and the standard source chain survive additional context.
ResultExt accepts foreign standard errors. The library owns no runtime, process environment,
filesystem logger or signal handler. Use rivium-host to assemble those resources.

Requires Rust 1.92. See the repository architecture and upgrade guides for exact contracts.
Licensed under MIT or Apache-2.0.
