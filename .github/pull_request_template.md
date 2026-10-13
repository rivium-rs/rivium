## Summary

<!-- What changes and why; link the issue or discussion. The title is a Conventional Commit
and becomes the squash commit and generated release-note entry. See docs/contributing/pull-requests.md. -->

## Verification

<!-- List exact commands, targets/features and results; distinguish local results from CI.
Record checks not run and any required maintainer-local or manual platform acceptance. -->

## Checklist

- [ ] The title and every commit follow Conventional Commits
- [ ] Relevant development checks are recorded, including anything not run (`docs/development.md`)
- [ ] Bug fixes have regression coverage; each new guard was shown to fail on a planted violation
- [ ] API, configuration, dependency/MSRV and platform impacts are noted where applicable
- [ ] README, API docs and `docs/` are updated where behavior changed
- [ ] Public-content hooks ran, or this fork PR explicitly awaits the maintainer's local check

<!-- Missing local checks are not waivers: required CI and manual acceptance must pass before merge. -->
