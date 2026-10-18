# Architecture

This guide describes the released 0.2.0 rewrite. It is an implementation contract,
not a claim that every deployment platform has been verified.

## Crates

```text
application --> rivium-host --> rivium-util --> rivium
     |                `------------------------^
     `-------> rivium-http ---------------------^
JNI library -> rivium-jni -> rivium-host
tests ------> rivium-test -> rivium
```

The core defaults to dependency-free errors. `task` adds lifecycle registration;
`rt` adds supervision. Utilities are selected independently. The host owns the runtime,
environment and logging. HTTP and JNI do not depend on each other. Test helpers never
belong to a production dependency closure. TLS is supplied through the application's
listener adapter, not a normal dependency of rivium-http.

## Services and task ownership

`Service` supplies its name, kind and an owned future. `service` adapts a closure;
`periodic` constructs a background service with an immediate first tick and Skip behavior.
All roots report `Context::ready()` before Phase::Running. Health can independently keep
readiness false; it does not prevent an initialized embedded start from returning.
Health names are unique, at most 128 bytes, registered only during startup, with 4096 entries.

`Supervisor::run()` returns `RunReport { outcome, drain }`. An outcome says why control
ended; a drain says whether registered execution bodies released their captures.
Returned values, unregistered tasks and external device operations are outside that proof.
Started blocking closures and non-yielding code cannot be forcibly killed.

Each service has a child TaskScope; roots are tracked separately. `reserve` and `close`
have one order. Reservations acquired before closure remain valid and already count toward
drain, including unused reservations. `cancel` also closes admission. `drain(within)`
only observes; it does not request stop. Captures are released before registration counts.
No application factory, destructor or waker runs under the admission lock.

Context submissions acquire both a task slot and a service-local name before returning
success. Duplicate/empty names, overload and closure have typed SubmitError values.
There are at most 4096 supervisor-owned roots/children and 4096 queued commands.
Do not turn Closed during ordinary shutdown into a business failure using an implicit conversion.

Shutdown closes/cancels frontline scopes first. After their roots and descendants drain,
background scopes close/cancel. One deadline covers both phases. Deadline or second stop
requests abort, followed by one additional 100 ms confirmation budget for the whole batch.
The report retains unconfirmed work; Phase::Stopped alone is not permission to close a database.

## Persistent task sets

`rivium-util/task` separates pure planning, owned reconciliation and an optional driver.
`Manager` owns one incarnation per key. Its capacity counts every retained record, including
backoff/exhausted/stopping records. Inputs exceeding capacity or containing duplicate keys
are rejected before destructive work. The driver reports DesiredRejected and preserves tasks.

All candidate admissions are reserved before stops. Replacement waits for actual termination;
unconfirmed work keeps its handle and slot. Cancelling reconciliation preserves stopping state
for a later cleanup attempt. Closing a manager that borrows a service scope does not close sibling
managers. Drop requests abort but cannot await completion.

Normal completion restarts a still-desired persistent task without a failure penalty. Failed/panicked
instances consume a budget; meaningful progress from that live incarnation resets its streak.
A completed incarnation's retained Context cannot reset a new instance. Defaults are 1024 keys,
5 consecutive failures, 5 seconds times 2^failures capped at 300 seconds, and a 3-second stop budget.
Source notifications are coalescing hints; the source and periodic fallback remain authoritative.

## Configuration and files

Loader precedence is defaults, TOML, explicit environment inputs, then explicit overrides.
The loader never reads process environment implicitly. Parent-section replacement also replaces
child raw values and provenance. Unknown keys and invalid values produce diagnostics; unusual
Serde flatten/untagged schemas can hide unknown keys and should be avoided for strict configuration.
Input defaults to at most 1 MiB and is checked while reading, before parsing an unlimited file.

Store has one writer and cloneable read handles. A Snapshot contains value and generation together;
use one snapshot for a logical operation. Host reload validates candidate and effective values,
then publishes under the same admission order as stop. Notifications and old-value destruction
occur outside locks. Publication is not acknowledgement by every consumer or a transaction over
external resources. Hot fields must actually be read by running consumers. Host log/lifecycle
fields remain cold and take effect on the next generation.

`atomic_write` uses a same-directory temporary file, file sync, optional backup, rename and Unix
directory sync. Its typed error records the failed stage and whether replacement already committed.
A post-rename directory-sync failure does not mean the old file remains. Concurrent writers,
path authorization and database migrations belong to the application.

## Logging

Logger owns a scoped tracing Dispatch; no global subscriber or panic hook is installed.
File formatting is capped at 64 KiB per event, with a queue of 1024 events. Oversized events are
dropped whole; queue-full drops and IO failures are counted separately. Arbitrary user Display/Debug
code and synchronous console/logcat IO cannot be bounded by the file queue.

Default disk budgets are 16 MiB per file, 256 MiB total and a 64 MiB export reservation when enabled.
At most 16 named categories are allowed, subject to the tighter disk-budget formula. ASCII names,
unique categories, valid filters and checked budget arithmetic are validated before use.
An incremental archive ledger avoids per-event directory scans. Raw/gzip archives and ZIP output
retain their accounting; owned temporary files are cleaned without deleting unrelated files.
Retention also bounds active/raw/gzip files to 8192 across all sinks. Rotation evicts the
oldest archive sequences before reserving two file slots: one for the new archive and one
for the raw/gzip overlap during compression. Byte limits and file-count limits share the
same eviction policy; deletion failures remain errors. Startup still rejects an already
over-limit directory through its bounded scan rather than deleting history to recover it.

Close rejects new work, cancels export and waits within one shared budget. Workers hold the directory
lease until actual release. A timeout can be retried, and late IO failures remain observable even
when no worker remains. Logs are lossy diagnostics, not a durable audit/event queue.

## Hosts

`rivium_host::App` defines identity, a typed configuration, validation, pure reload policy and service
construction. It does not spawn during construction. Context exposes the running configuration,
paths, health, control and optional exporter. Its generation-bound `HostControl` closes reload
admission synchronously when stop or restart is requested, including during construction;
a retained control from an old generation cannot stop a later generation's reloads.
Config file writes, reload and restart are separate actions.

A host session owns a runtime. Each internal restart gets a new generation with fresh config,
services and logger; old services, reload work and log workers must release before replacement.
`HostReport` preserves the primary error separately from `CleanupReport { closed, pending, failures }`.
Closure plus zero pending resources proves a drain; no recorded cleanup failures is an additional
condition for success. `Host::drain` observes the generation captured by its call, without stopping it.

The process host reads its environment and handles SIGINT/SIGTERM; SIGQUIT retains the OS default.
The embedded host installs no signals and requires an explicit root. `stop(within)` spends one total
caller budget; zero requests stopping and returns promptly. An already-finished host returns NotRunning.
Rust/JVM callers must not run blocking start/stop methods on latency-sensitive UI threads.

Process exit codes remain 0 success, 64 usage, 69 startup, 70 fault, 71 OS, 73 log creation,
75 requested restart, 78 config, 124 stop timeout, and 128+signal for a second signal.
Default startup is 30 seconds, service grace 3 seconds and process restart mode exit.
In-process recovery uses bounded backoff and refuses to overlap generations.

## HTTP and JNI

HTTP default builds provide response vocabulary and extractors; contract adds request IDs,
handler timeout, body checks, access observers and request admission. Server adds owned listeners
and probes; fs is separately selectable. CORS/compression belong outside the contract.

Defaults are 256 connections, 256 requests, 30 seconds to response headers and 2 MiB request bodies.
Connection permits precede accept. Request permits are shared by handler, request body and response
body until the last owner releases; Tower's response-future limiter alone cannot provide this lifetime.
Full connections stop accept; full request admission returns 503. A server binds and reports its own
readiness, then waits for all roots to initialize before accepting business work.

Declared oversized Content-Length is rejected before the handler, with u64 checking on 32-bit targets.
Unknown-length data can only be limited when read: collecting extractors can return 413, raw streams
surface body errors, and sent response headers cannot be replaced. Handler timeout does not bound the
entire response stream. Streaming handlers must cooperate with stop or remain visible as incomplete.
Custom TLS listeners must bound their own hidden work and be cancellation-safe; background prefetch
cannot be constrained automatically by an outer accept permit.

Error diagnostic context is private. HTTP only uses explicit public_message for eligible caller 4xx;
401/403, upstream and server failures use safe titles/status phrases. NoEnvelope keeps application
protocol shapes but does not bypass resource limits.

JNI's thin export macro generates the required loader entry; normal Rust functions own all lifecycle
behavior. Five static methods preserve their ABI. Integer panics return -99, string panics empty strings;
negative stop durations clamp to zero. Java receives safe error summaries. No new main/test/service/config
macro framework is included; those operations use normal APIs and Tokio/Serde mechanisms.
