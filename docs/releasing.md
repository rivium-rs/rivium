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
  belong to `origin/main`, match all five public crates and their internal dependencies, and not
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
cargo package -p rivium-error -p rivium -p rivium-http -p rivium-jni -p rivium-test \
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
archives and compare all five packages plus the notes/metadata, not arbitrary files in `target`.
A dry run never needs `--allow-dirty`; change packaged source on a dedicated committed candidate
if Cargo reports it dirty.

The semver job checks every public crate against its published baseline, with all features. This
plugin has no `--locked` switch; the workflow checks that the root lockfile is unchanged. Ordinary
Cargo packaging/build commands remain locked. Missing baselines and registry errors are failures,
not first-release exceptions.

## Protected publishing and recovery

The repository must have the `PUBLIC_DENYLIST` secret and a **protected `crates-io` environment**
with a required reviewer and a deployment policy allowing the version tags. Configure Trusted
Publishing for each of the five existing crates, bound to this repository, `release.yml` and
that environment. These are maintainer-managed settings; adding the workflow does not set them.
The workflow checks reviewer protection through the read-only environments API. It has no
long-lived-token fallback and does not provision new crate names.

The publish job is the only job with `id-token: write` and `contents: write`. After every required
job succeeds and environment approval is granted, it checks out the same tag, installs the same
Rust version, downloads the verified artifact **by ID from that run**, and compares freshly
packaged bytes against it. Dependency build scripts do not run in this privileged job:
`--no-verify` is used only after the identical archives passed the unprivileged `publish --dry-run`.
An explicit OIDC action obtains a short-lived token; only then does Cargo publish the workspace.
The workflow checks registry checksums and then creates and reads back the GitHub Release, using
the generated notes file rather than interpolating its contents into a shell command.

Publications are serialized and never automatically replayed after an ambiguous failure. If some
crates were uploaded, an API times out, or the GitHub announcement fails, **inspect remote state
first**. The read-only `scripts/release-registry.sh published <version> <verified-artifact-dir>`
checks all five uploaded versions against the retained archives. A partial publication needs a
maintainer-directed recovery; do not delete/recreate tags, assume rollback, or blindly rerun an
upload. If all crates are verified and only the announcement is missing, recover the announcement
without uploading the crates again. Preview artifacts are retained for seven days; expired or
incomplete evidence requires fresh validation, not bypassing the checks.
