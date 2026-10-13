# Reviewing changes

[Contribution guide](README.md) · [Pull request workflow](pull-requests.md)

You can help review a PR without being a maintainer. Reproducing a bug, checking one platform,
reading an example or noticing an unclear API is useful. State the scope of your review;
community feedback does not replace the maintainer's approval or required checks.

## What to review

Start with the issue and PR description, then read the changed code and the callers or tests
that rely on it. Focus on these questions before style details:

- **Scope:** does the change solve the stated problem, and does it belong in Rivium rather than
  application code? Are unrelated refactors or options making it harder to assess?
- **Correctness:** what happens on failure, shutdown, restart and repeated calls? Are lifecycle
  ordering, readiness, deadlines, cleanup and error reporting preserved where affected?
- **Compatibility:** do public APIs, configuration defaults, error contracts or target support
  change? Are breaking changes and migration steps visible?
- **Dependencies:** are crate boundaries, feature isolation, MSRV and package contents preserved?
  Use the [dependency guide](dependencies.md) when manifests or build behavior change.
- **Evidence:** does a regression test cover the original bug? Do new guards fail on a planted
  violation? Are relevant failure paths tested, rather than only the successful case?
- **Documentation:** can a user follow the example or upgrade instructions? Do platform claims
  match the [recorded verification](../platforms.md), not merely a configured job?

For JNI changes, inspect error and panic containment as well as loading and repeated start/stop.
For workflow or release changes, examine permissions, untrusted inputs, artifact identity and
failure recovery; a passing fixture alone does not establish safe publication. Use the existing
[release procedure](../releasing.md) rather than weakening a gate to make the PR green.

## Checking the evidence

Say what you actually inspected or ran, with the revision, command, target and result when
relevant. Read failing or skipped checks before concluding the PR is ready. Native execution,
emulation, compile-only checks and a real device run establish different things.

A fork PR needs the maintainer's local public-content check. Missing manual hosting or device
evidence stays missing even if the available CI jobs pass. Do not report a check as passed
because a related or historical run passed.

For performance claims, ask for a reproducible before/after measurement of the affected path,
with the same environment and workload. A new benchmark framework is not required for a change
that makes no performance claim.

## Giving useful feedback

Identify the location, the behavior that concerns you and why it matters. Give a concrete input,
execution sequence or expected result where possible. Separate a blocking correctness issue
from an optional improvement or a question. If an existing tool enforces formatting, use its
result rather than introducing a second style rule in review.

Keep feedback about the work, not the contributor. Suggest a way to investigate or test a
concern, and be willing to revise your conclusion when new evidence arrives. Acknowledge
resolved comments and avoid requiring unrelated cleanups before a focused fix can land.

## Updates and stalled work

Authors should answer feedback and push follow-up commits without rewriting published history.
After an update, reviewers should recheck the affected behavior and evidence rather than
assuming the earlier review covers new changes. Maintainers handle the final
[merge checklist](pull-requests.md#merge-maintainer-checklist).

If work stalls, ask whether the author wants help before taking it over. Link the original PR,
credit the work you reuse and respect the repository's public identity requirements. Leave a
short handoff with completed work, open questions and checks still needed. There is no fixed
response deadline implied by this guide.
