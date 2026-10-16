# Pull requests

[Contribution guide](README.md) · [Development checks](../development.md) · [Reviewing](reviewing.md)

A pull request should be one coherent, reviewable change. Small fixes and documentation changes
can start here; discuss substantial changes in an [issue](issues.md#proposing-a-feature) first.
Run shell commands below from the repository root unless a step says otherwise.

## Get the source

If you do not have write access, fork the repository and clone your fork. Replace
`YOUR-USERNAME` with your GitHub account:

```bash
git clone https://github.com/YOUR-USERNAME/rivium.git
cd rivium
git remote add upstream https://github.com/rivium-rs/rivium.git
git fetch upstream
git switch -c fix/short-topic upstream/main
```

If you already have a checkout, inspect its remotes and working tree before creating a branch;
do not discard unrelated changes. Contributors working directly in the main repository use
its `main` branch as the starting point instead of a fork's upstream remote.

Use `<type>/<topic>` for branch names, such as `fix/log-rotation` or `docs/configuration`.
Initial-plan development units use `c-<n>/<topic>`. Use one branch and one PR per unit, and
start the next unit from `main` once the previous one is merged. If a unit must depend on a
PR still under review, branch from that unit; after it merges, merge `main` into the dependent
branch before requesting review. Do not rebase it or rewrite pushed history.

Install the [local development tools](../development.md) under `.tools/`, not globally.
Workspace tests require a JDK. Contributors with the private denylist must also install the
[public-content hooks](#public-content-checks); fork contributors use the maintainer-check path.

## Implement the change

Keep changes related to the stated problem. Do not mix in unrelated formatting, dependency
updates or generated files. Consult the [crate boundaries](../architecture.md#crates) and the
[dependency guide](dependencies.md) before moving responsibilities between crates.

For a bug fix, add a regression test that demonstrates the original failure and passes with
the fix. Test normal and failure paths for new behavior. Update API documentation, examples,
configuration guidance and migration instructions wherever the behavior they describe changes.
New verification guards must be shown to reject a planted violation, not just accept good input.

## Checks

The [development guide](../development.md) is the source for commands and pinned tool versions.
Use this table to find additional checks for your change; it does not waive required CI:

| Change | Relevant verification |
| --- | --- |
| Library behavior or lifecycle | Native workspace tests and affected process or embedded contracts |
| Public API, features or dependencies | [Feature and supply-chain checks](../development.md#native-and-feature-contracts), MSRV and [dependency review](dependencies.md#verification) |
| JNI bridge or loading | [JVM loader contracts](../development.md#jvm-and-platform-boundaries); Android checks when affected |
| Linux ABI, linker or cross-target behavior | [glibc](../development.md#jvm-and-platform-boundaries) and [extended-platform checks](../development.md#jvm-and-platform-boundaries) |
| Verification scripts or workflows | Shell lint and [guard self-tests](../development.md#verification-tooling-and-publication), plus real execution where the claim needs it |
| Documentation only | `git diff --check`, local links and anchors, command/API examples and the public-content check |

Prebuild the test programs before process tests, using the same toolchain, target, build profile
and target directory. Record exact commands and results in the PR, including skipped checks
and missing tools. Fixture success is not real platform verification; a desktop JVM is not an
Android device. Required CI and manual acceptance still apply even when you cannot run them locally.

## Commits and opening the PR

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
<type>(<scope>)!: <subject>
```

The scope and `!` are optional. Allowed types are `build`, `chore`, `ci`, `docs`, `feat`, `fix`,
`perf`, `refactor`, `revert`, `style` and `test`. Use an imperative subject, at most 72 characters,
with no trailing period. Explain why in the body; identify breaking changes explicitly.

For example:

```text
fix(log): preserve the disk budget during rotation
docs(contributing): explain the fork workflow
```

Inspect `git diff` and the staged diff before committing. Push the branch and open a **draft PR
against `main` early**. Use the [PR template](../../.github/pull_request_template.md) to record:

- What changes, why it is needed and the related issue or discussion.
- The commands run and their results, including relevant targets and features.
- Checks still needed, compatibility changes and documentation updates.
- Any maintainer-only public-content or platform acceptance work remaining.

The PR title is also a Conventional Commit. Leave room for GitHub's ` (#<n>)` suffix within
72 characters; `check/pr-title` checks the combined title. It becomes the squash commit and
therefore the generated release-note entry, so keep it meaningful to a user of the crate.

## Discuss and update

Respond to review comments with the change made, supporting evidence or a question. You can
disagree with a suggestion; explain the trade-off. Push follow-up commits rather than amending,
rebasing or force-pushing published history. Merge updated `main` when needed, resolve conflicts
and rerun checks affected by the update.

The `ci` workflow runs on PRs and `main`; public-content checks run on repository pushes and
eligible PRs. Mark the PR ready once the work is complete and required checks pass, with any
fork-only private check clearly assigned to the maintainer. A green CI result alone is not
approval to merge. See the [review guide](reviewing.md) for what reviewers look for.

## Public-content checks

Contributors with access to the private denylist configure the hooks once:

```bash
git config core.hooksPath scripts/hooks
git config rivium.denylist /absolute/path/to/private-denylist
```

The hooks inspect each commit's staged tree, identity and message, and each push. Commit
messages must also follow Conventional Commits. Keep the denylist outside the repository;
do not commit it, disclose its entries or replace it with an empty list.

Fork PRs receive no repository secrets, so their public-content job is skipped. State that in
the PR; the maintainer must check the proposed contribution locally with the real denylist
before merging. Scan the fork branch's reachable history, not only its final diff. From that
branch's checkout, invoke a trusted copy of the scanner from reviewed `main`:

```bash
/path/to/trusted-checkout/scripts/check-public.sh "$(git config --get rivium.denylist)"
```

Do not expose the private denylist to an unreviewed checker or build script from the PR.
A skipped workflow or a successful scan against a substitute list is not evidence that the
required check passed. Do not bypass a failing check. The maintainer also checks the final
merge message below.

## Merge: maintainer checklist

Only merge after the maintainer approves this PR and the required checks, including any
fork-local public-content check, are complete. Use **Squash and merge**, not a rebase merge.
The final title must describe the reviewed change and fit the commit-message rules.

GitHub merges do not run local hooks. Using the same trusted scanner, check the final message
with `scripts/check-public.sh <denylist> --message <file>` and use a GitHub noreply author address.
The merge command is:

```text
gh pr merge <n> --squash --delete-branch --subject "<title> (#<n>)" --body-file <file> --author-email <noreply>
```

`--delete-branch` removes the remote and local branch; the PR keeps its commits. Never force-push
or rewrite pushed history. Publishing is a separate, maintainer-only operation covered by the
[release guide](../releasing.md); merging a PR does not itself publish a release.
