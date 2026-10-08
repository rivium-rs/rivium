# Contributing

## Workflow

1. **Branch from `main`.** One branch per change, named `<type>/<topic>` (for example
   `fix/log-rotation`); a development phase of the initial plan uses `c-<n>`.
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
   push.
5. **When the work is complete,** run `just check`, make sure every check is green and mark the
   pull request ready for review. Its title is a Conventional Commit (`check/pr-title`).
6. **Merge with "Squash and merge"** after the maintainer approves. The pull request title becomes
   the commit on `main`. Merge commits made on GitHub skip the local hooks, so check the final
   message with `scripts/check-public.sh <denylist> --message <file>` and merge with your
   GitHub noreply address as the author, for example
   `gh pr merge <n> --squash --subject "<title> (#<n>)" --body-file <file> --author-email <noreply>`.
   Keep the branch; never force-push or rewrite history that has been pushed.

## Checks

- New checks are shown to fail once on a planted violation before they are relied on.
- Developer tools are installed under `.tools/` (`just tools`), never globally.
- Releases are tagged by the maintainer only.
