# Releasing Rivium

[Contributing](contributing/README.md) · [Development checks](development.md) ·
[Platform support](platforms.md)

This guide is for maintainers. Run commands from the repository root. A successful local
preview is not a release, and automated checks do not replace the required manual acceptance.

- [Release policy](#release-policy)
- [Local preview](#local-preview)
- [Protected publishing and recovery](#protected-publishing-and-recovery)

## Release policy

Releases are made by the maintainer only. Every crate shares one version, and the release notes
are generated, never written by hand: git-cliff renders them from the commits on `main` since the
previous tag (`cliff.toml`), and those commits are the pull requests' titles, so a title is also
the line that the release notes show. `git cliff --unreleased --tag v<version> --strip all`
previews them.

The release workflow has two deliberately different entry points:

- **Manual dispatch is always a dry run.** It runs native CI, the extended daily matrix,
  public-content checks, API compatibility, package verification and generated notes. It has no
  publishing token and creates neither a tag nor a GitHub Release. While the workspace still
  names an existing release, notes preview the next patch; packages keep the actual workspace
  version. This preview does not decide the appropriate next version.
- **A push of a stable `vX.Y.Z` tag requests publication.** The tag must identify the checkout,
  belong to `origin/main`, match all six public crates and their internal dependencies, and not
  already exist on crates.io. A new minor/major needs its upgrading chapter. Every version needs
  its own verification section in `docs/platforms.md`; its presence is a record requirement,
  not automatic proof of the claims in it.

Before tagging, the maintainer chooses and aligns the version and lockfile, reviews the generated
notes, and records the actual verification method, source revision and results. Do not relabel
historical results or unverified devices/hosting as passed. In particular, the current daily
workflow does not implement real systemd/launchd/WinSW hosting or sd-notify verification. The
`crates-io` environment reviewer must resolve the release's required manual acceptance before
approving it; T3 targets without a record remain explicitly unverified under the platform policy.

## Local preview

Install `git-cliff` 2.14.2 and `cargo-semver-checks` 0.51.0 under `.tools`, and put `.tools/bin` on
`PATH`. The private denylist configured for the hooks is also required for package scanning.
The output directory must be scratch space, not a directory containing unrelated package files:

```bash
work=$(mktemp -d)
scripts/release-check.sh preview "$work/release"
cargo package -p rivium -p rivium-util -p rivium-host -p rivium-http -p rivium-jni -p rivium-test \
  --all-features --locked --registry crates-io --no-verify --target-dir "$work/release/build"
scripts/release-packages.sh scan "$work/release/build/package" "$work/release" \
  "$(git config --get rivium.denylist)" "$work/verified"
cargo publish --dry-run --workspace --all-features --locked --registry crates-io \
  --target-dir "$work/release/build"
scripts/release-packages.sh verify "$work/release/build/package" "$work/release" "$work/verified"
```

The explicit package list is intentional: `cargo package --workspace` also attempts to package
private members. Workspace publishing, by contrast, selects publishable crates and orders their
dependencies. The helpers accept Cargo's known workspace staging layout, reject ambiguous/missing
archives and compare all six packages plus the notes/metadata, not arbitrary files in `target`.
A dry run never needs `--allow-dirty`; change packaged source on a dedicated committed candidate
if Cargo reports it dirty.

The semver job checks the four continuing packages (rivium, rivium-http, rivium-jni,
and rivium-test) against published baselines. rivium-util and rivium-host are new package
identities and use package/feature/MSRV verification for their first release. The checker
has no --locked switch; CI verifies that the root lockfile stays unchanged. Missing existing
baselines and registry errors remain failures. New package names require the first-publication
setup below; a dry run does not verify publishing credentials or Trusted Publisher settings.

The release aggregate requires the dedicated Loom job for the same candidate as well as
native/daily/package/content checks. Failure, cancellation or a skipped prerequisite is
not a successful release gate.

## Protected publishing and recovery

The repository must have the `PUBLIC_DENYLIST` secret and a **protected `crates-io` environment**
with a required reviewer and a deployment policy allowing the version tags. Publishing normally
uses Trusted Publishing for each of the six public crates, bound to `rivium-rs/rivium`,
`release.yml` and that environment. The `v0.2.0` tag uses the one-time API token setup below
because Trusted Publishing cannot be configured until a crate exists. These are maintainer-managed
settings; adding the workflow does not set them. The workflow checks reviewer protection through
the read-only environments API. Other tags require OIDC and have no API-token fallback.

The publish job is the only job with `id-token: write` and `contents: write`. After every required
job succeeds and environment approval is granted, it checks out the same tag, installs the same
Rust version, downloads the verified artifact **by ID from that run**, and compares freshly
packaged bytes against it. Dependency build scripts do not run in this privileged job:
`--no-verify` is used only after the identical archives passed the unprivileged `publish --dry-run`.
For `v0.2.0`, Cargo uses the environment's `CARGO_REGISTRY_TOKEN` secret; for other tags, an explicit
OIDC action obtains a short-lived token. Only the publish step receives the selected credential.
Manual previews receive neither publishing credential and cannot publish.
The workflow checks registry checksums and then creates and reads back the GitHub Release, using
the generated notes file rather than interpolating its contents into a shell command.

### One-time publication of new crate identities

`rivium-util` and `rivium-host` are new crate identities in this release. They do not have
published semver baselines; the package, feature, MSRV, archive and dry-run checks are their
first-release validation. They are still part of the same six-package publication and are
published in Cargo's dependency order.

Before creating `v0.2.0`, create one crates.io API token and store it as `CARGO_REGISTRY_TOKEN`
in the protected `crates-io` environment. Give it a short expiry and the least permissions that
allow publishing all six public packages, including creating `rivium-util` and `rivium-host`.
Separate tokens per crate are unnecessary. A token restricted to existing crate names will not
cover these new identities. The workflow uses this secret only for the exact `v0.2.0` tag;
manual dispatch remains an unprivileged preview.

Do not manually publish either new crate's `0.2.0` version first. The tag workflow requires a
read-only `absent` check for all six names before publication; an already uploaded version fails
that check. Cargo publishes all six packages in dependency order after the common checks and
environment approval.

After all six published checksums are verified, configure Trusted Publishing for every package,
including the two newly created crates: repository `rivium-rs/rivium`, workflow filename
`release.yml` (the file at `.github/workflows/release.yml`), environment `crates-io`. Revoke the
API token on crates.io and remove the environment secret. Subsequent version tags use OIDC;
removing the secret does not make `v0.2.0` safe to replay. A dry run proves neither token
permissions nor publisher configuration.

Workspace publication is not transactional. Publications are serialized and never automatically
replayed after an ambiguous failure. If some crates were uploaded, an API times out, or the GitHub
announcement fails, **inspect remote state first**. The read-only
`scripts/release-registry.sh published <version> <verified-artifact-dir>` checks all six uploaded
versions against the retained archives. A partial publication needs a
maintainer-directed recovery; do not delete/recreate tags, assume rollback, or blindly rerun an
upload. If all crates are verified and only the announcement is missing, recover the announcement
from the retained `notes.md` without uploading the crates again. Preview artifacts are retained
for seven days; expired or incomplete evidence requires fresh validation, not bypassing the checks.
