# Contributing to Rivium

Thanks for helping improve Rivium. Contributions can be bug reports, clearer documentation,
reproductions, tests, code or reviews. You do not need to understand the whole workspace to
make a useful contribution.

Start with the [contribution guide](docs/contributing/README.md). It has separate paths for
[issues](docs/contributing/issues.md), [pull requests](docs/contributing/pull-requests.md),
[reviews](docs/contributing/reviewing.md) and [dependency changes](docs/contributing/dependencies.md).
The policies below apply across those paths.

## Working together

Keep discussions respectful and specific. Explain the behavior or trade-off you are concerned
about, not assumptions about the person proposing it. Distinguish a required change from an
optional suggestion, and help new contributors find the relevant code or test.

Discuss substantial API, lifecycle or platform changes in an issue before implementing them.
Small fixes and documentation improvements can go directly to a pull request.

## Getting help

Check the [architecture guide](docs/architecture.md) and [API documentation](https://docs.rs/rivium)
first, then [open an issue](https://github.com/rivium-rs/rivium/issues) if something is unclear.
Include a small example and the version and platform involved. The
[issue guide](docs/contributing/issues.md) explains what makes a useful report and how to help
with someone else's report. Do not put credentials, private logs or sensitive reports in a
public issue.

## Workflow

Use one branch and one pull request per coherent change. Commits and PR titles follow
Conventional Commits; reviewed changes are squash-merged by a maintainer. Do not force-push
or rewrite history that has already been pushed.

See the [pull request guide](docs/contributing/pull-requests.md) for fork setup, branch rules,
commit examples, review updates and the maintainer's merge checklist.

## Checks

Run the [development checks](docs/development.md) relevant to your change and record the actual
commands, results and anything not run. Add regression tests for fixes and update documentation
when behavior changes. New checks must fail on a planted violation before they are relied on.

Public-content checks use a private denylist. Fork pull requests cannot receive its secret;
the maintainer performs that check locally before merging. A skipped check is not a pass.
See [public-content checks](docs/contributing/pull-requests.md#public-content-checks) for setup
and the fork workflow.

## Minimum supported Rust version

Rivium's minimum supported Rust version (MSRV) is **1.92**, declared in the workspace manifest
and tested by `ci/msrv`. Raising it requires the maintainer's approval, including when the
change comes from a dependency update. A successful build on stable alone does not verify MSRV.

## Versioning

The six public crates are released together at one version. Before 1.0, a new minor release
may change the API or configuration contract; those changes need migration steps in the
[upgrade guide](docs/upgrading.md). Keep breaking changes explicit in the PR and its title.
The maintainer chooses the next version; do not bump it as part of an unrelated contribution.

Platform claims are tied to a version and its [verification record](docs/platforms.md), not
to the presence of a workflow. This guide does not establish an LTS or backport support window.

## Releases

Releases are maintainer-only. Notes are generated from reviewed commit titles. Manual dispatch
is a non-publishing preview; a stable `vX.Y.Z` tag requests publication only after the required
checks, evidence and protected-environment review. See the [release guide](docs/releasing.md)
for the complete procedure, including recovery after partial publication.

## Contribution license

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this project by you, as defined in the Apache-2.0 license, shall be dual licensed under
[Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option,
without any additional terms or conditions.
