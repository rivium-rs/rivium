# Dependencies and features

[Contribution guide](README.md) · [Architecture](../architecture.md#crates) ·
[Development checks](../development.md)

Dependency changes affect published consumers, MSRV and cross-target builds, not just the
current checkout. Explain why an existing helper or dependency is insufficient, which crates
need the addition and which features and platforms it affects.

## Workspace declarations

Declare shared third-party versions in the root [`Cargo.toml`](../../Cargo.toml). The workspace
policy disables their default features; each member enables only the features it uses. For
example, the existing serde declarations have this shape:

```toml
# Root Cargo.toml
[workspace.dependencies]
serde = { version = "1.0.219", default-features = false }
```

```toml
# A member's Cargo.toml
[dependencies]
serde = { workspace = true, features = ["std", "derive"] }
```

Do not scatter independent versions across member manifests or enable broad feature sets to
make one build pass. When newly used APIs require a newer dependency version, update the
manifest requirement as well as the lockfile; a newer resolved lockfile alone does not express
that requirement to consumers. Keep version and feature changes tied to the actual need.

## Public crates and boundaries

All five public Rivium crates inherit the same workspace version. Their normal/build Rivium
dependencies use the workspace's local path and matching published version. Keep both aligned
for a release; do not replace a publishable dependency with a path-only or private workspace
member dependency. The [release guide](../releasing.md) owns release version changes.

Keep the existing separation:

- The core `rivium` crate does not pull HTTP or JNI into a basic service.
- `rivium-http` and `rivium-jni` do not depend on each other.
- `rivium-test` is test support, used as a dev-dependency, not part of the production dependency
  closure of a service.
- `log-export` is opt-in. Features are additive; enabling one must not remove behavior expected
  by another consumer.

Some library tests use a direct path-only **dev-dependency** on `rivium-test` to avoid a publish
ordering cycle. That existing exception is not permission to make normal/build dependencies
path-only. Keep `test-app`, `test-jni`, `tests-integration` and `tests-build` private.

## Lockfile and MSRV

Commit intentional `Cargo.lock` changes with the manifest or dependency change that needs them.
Review the diff for unrelated upgrades and explain them if unavoidable. Routine validation uses
`--locked`: do not delete or regenerate the lockfile just to make a check pass, and do not
rewrite manifests or remove dev-dependencies for a feature check.

The workspace uses resolver 3 and Rust **1.92** as its MSRV. A dependency upgrade that raises
the effective MSRV needs the same maintainer approval as changing `rust-version`; newer stable
Rust alone is not sufficient verification. Check target-specific and build dependencies too,
not only the direct runtime dependencies of your host platform.

## Verification

After [installing the local tools](../development.md#setup), run these from the repository root:

```bash
cargo hack --each-feature --workspace check --locked
cargo test --package tests-build --locked
cargo deny --locked check
```

The build tests inspect full normal/build dependency closures, JNI compile guards, consumer
APIs, package contents and library code budgets. A feature check uses normal build targets;
it does not need edited manifests or a regenerated lockfile. Use the
[native checks](../development.md#native-checks) and affected target lanes as well.

To reproduce the MSRV lane with Rust 1.92 and a JDK installed, keep the same feature set in
the prebuild and test run:

```bash
cargo +1.92 build --package test-app --bins --all-features --locked
cargo +1.92 test --workspace --all-features --locked
```

Inspect the public manifest's `include` list when adding files. Repository-only fixtures,
scripts and internal documents must not become part of a published library package. Do not
relax dependency guards, package boundaries, deny rules or code budgets merely to accept a
new dependency; make any intended policy change explicit for review.
