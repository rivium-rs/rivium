# Contributing

Use this guide as a map, not a list of documents you must read before helping. The
[root contribution page](../../CONTRIBUTING.md) covers shared policies, MSRV and versioning;
the pages here explain how to take part.

## Quick start

- **Ask a question or report a bug:** follow [the issue guide](issues.md#reporting-a-bug).
- **Find something to work on:** check [existing issues and ongoing work](issues.md#finding-and-tracking-work).
- **Make a change:** start with [fork and branch setup](pull-requests.md#get-the-source).
- **Help review a change:** use the [review checklist](reviewing.md#what-to-review).
- **Add or update a dependency:** check [dependency and feature rules](dependencies.md).
- **Prepare a release:** use the [maintainer release guide](../releasing.md).

## Guides

- [Issues and questions](issues.md): reproductions, feature proposals, triage and tracking work.
- [Pull requests](pull-requests.md): setup, implementation, checks, commit messages, review and merge.
- [Reviewing changes](reviewing.md): correctness, test evidence, constructive feedback and stalled work.
- [Dependencies and features](dependencies.md): workspace manifests, crate boundaries, lockfiles and MSRV.
- [Development checks](../development.md): pinned tools, native tests, JVM, glibc and cross-target checks.
- [Release procedure](../releasing.md): dry runs, approval, publication and recovery.

## Repository map

| Location | Purpose |
| --- | --- |
| [`crates/`](../../crates) | The five public libraries; see the [architecture guide](../architecture.md#crates) for their boundaries |
| [`test-app/`](../../test-app) | Private host programs and the JNI library used by validation |
| [`tests-integration/`](../../tests-integration) | Process-level integration contracts |
| [`tests-build/`](../../tests-build) | Consumer, dependency, JNI compile, package-boundary and code-budget checks |
| [`scripts/`](../../scripts) | Verification drivers, public-content hooks and release helpers |
| [`scripts/tests/`](../../scripts/tests) | Self-tests that exercise guards and their failure paths |
| [`docs/`](..) | Architecture, upgrades, platform evidence and contributor guides |

For a first contribution, a reproducible bug report or a documentation correction is enough.
You do not need access to every supported platform; say which checks you could run, and keep
missing verification visible for the reviewer.
