# Contributing

## Workflow

1. **Branch from `main`, one branch and one pull request per coherent unit of work** (a crate, a
   module or a set of related changes), named `<type>/<topic>` (for example `fix/log-rotation`).
   A development phase of the initial plan is delivered as several such units on branches
   `c-<n>/<topic>`. Start the next unit from `main` once the previous one is merged; if it has
   to build on a unit that is still under review, branch from that unit and, after its pull
   request is merged, merge `main` into the dependent branch (never rebase it) before asking for
   review.
2. **Install the hooks once:** `just hooks <denylist>`. They check every commit (staged tree,
   identity, message) and every push against the private denylist, and check that commit
   messages are Conventional Commits.
3. **Commit as [Conventional Commits](https://www.conventionalcommits.org/):**
   `<type>(<scope>)!: <subject>`, where scope and `!` (breaking change) are optional and the
   type is one of `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`,
   `style`, `test`. Imperative subject, at most 72 characters, no trailing period; the body
   explains why.
4. **Push and open a draft pull request against `main` right away.** The `ci`, `cross` and
   `hosting` workflows run on pull requests and on `main`; the public-content check runs on every
   push. On a pull request that changes only documentation (files under `docs/` and Markdown
   files at the top level, as decided by `scripts/docs-only.sh`), every job of those three
   workflows except `changes` is skipped; on `main` every job runs.
5. **When the work is complete,** run `just check`, make sure every check is green and mark the
   pull request ready for review. Its title is a Conventional Commit that still fits in 72
   characters once GitHub appends ` (#<n>)`; `check/pr-title` checks it that way.
6. **Merge with "Squash and merge"** after the maintainer approves that pull request. Its title
   becomes the commit on `main`, which keeps every commit there a reviewed, CI-verified state.
   Merge commits made on GitHub skip the local hooks, so check the final message with
   `scripts/check-public.sh <denylist> --message <file>` and merge with your GitHub noreply
   address as the author, for example
   `gh pr merge <n> --squash --delete-branch --subject "<title> (#<n>)" --body-file <file> --author-email <noreply>`.
   `--delete-branch` removes the remote and local branch; the pull request keeps its commits.
   Never force-push or rewrite history that has been pushed.

## Checks

- New checks are shown to fail once on a planted violation before they are relied on.
- Developer tools are installed under `.tools/` (`just tools`), never globally.

## Releases

Releases are made by the maintainer only. Every crate and the template share one version, and
the release notes are generated, never written by hand.

1. On a branch: set the version in the workspace `Cargo.toml` (`[workspace.package]` and the
   `rivium-*` entries of `[workspace.dependencies]`) and in `template/.rivium-template`, and for
   a new minor version the template's `rivium-* = "0.<minor>"` requirements and its steps in
   `docs/upgrading.md`. The `release` workflow checks it all as a dry run on the pull request
   (`scripts/release-check.sh`) and prints the release notes, which git-cliff renders from the
   commits since the previous tag (`cliff.toml`); `just release-notes <version>` shows them
   locally. The commits on `main` are the pull requests' titles, so a title is also the line
   that the release notes show.
2. Once that pull request is merged and `main` is green, tag the merge commit `v<version>` and
   push the tag. The `release` workflow checks again, runs the whole matrix, publishes the crates
   in dependency order and creates the GitHub release with the notes.
