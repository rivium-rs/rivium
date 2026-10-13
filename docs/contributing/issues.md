# Issues and questions

[Contribution guide](README.md) · [Pull requests](pull-requests.md)

## Asking for help

Start with the [architecture guide](../architecture.md) and [API documentation](https://docs.rs/rivium).
If they do not answer your question, [open an issue](https://github.com/rivium-rs/rivium/issues)
with what you are trying to build, the code you have tried and where the result differs from
what you expected. A question or a documentation gap is useful feedback, not a failed contribution.

## Reporting a bug

Search existing issues first. If the same problem is already reported, add new evidence there
rather than opening a duplicate. A useful report contains:

- The Rivium crate versions, or the source revision for a Git checkout.
- `rustc -Vv` output, target triple, operating system and relevant runtime versions.
- The host form: standalone process, desktop JVM or Android JNI. Include the service manager,
  JDK or Android API level where relevant.
- Enabled crate features and the smallest configuration or program that reproduces the problem.
- Exact commands and steps, expected behavior, actual behavior and sanitized output.
- Whether the failure is repeatable, and a last known working version if you have one.

For lifecycle problems, include the order of start, ready, stop or restart events and the
configured deadlines. For build failures, include the relevant compiler or linker error rather
than only the command's exit status. Distinguish a native run, emulator run and compile-only
check; the [platform guide](../platforms.md) describes those boundaries.

Reduce the example without changing the failure. If you cannot isolate it yet, share what you
know and say which details are missing. Avoid requiring access to private services or devices
without explaining a way to reproduce or inspect the behavior.

### Sensitive information

Issues, pull requests and CI logs are public. Remove credentials, private endpoints, customer
data and identifying log fields from examples; never include the private denylist. For a
security concern or other report that cannot safely be public, ask a maintainer to arrange a
private channel before sharing the sensitive details. Do not assume a public issue is private.

## Proposing a feature

Describe the problem before proposing an API: who needs it, what they do today, why that is
insufficient and the smallest change that would help. Note affected crates, compatibility,
platforms and alternatives, including keeping the behavior in application code.

Discuss changes to lifecycle behavior, public APIs, dependencies or platform commitments before
starting a large implementation. An issue discussion helps decide scope; it is not a promise
that a particular design will be merged.

## Helping triage

You can help without submitting code. Reproduce the report and post the version, target,
commands and result; reduce the example; find related tests; or explain a likely cause with
supporting evidence. Keep confirmed observations separate from guesses. If you cannot reproduce
it, describe your environment rather than concluding that the report is wrong.

Ask for specific missing information, not a full environment dump. Suggest an existing label
or related issue if useful; you do not need label permissions to help. Maintainers decide
labels and issue state. Do not assume a fixed label taxonomy or response deadline.

## Finding and tracking work

Check [open issues](https://github.com/rivium-rs/rivium/issues) and
[open pull requests](https://github.com/rivium-rs/rivium/pulls) before starting. Comment with
your intended scope if a discussion already exists, so another contributor can tell you about
work in progress. No particular issue label is required to propose a small fix.

Link the issue from the pull request and keep significant findings in that discussion. Use
`Fixes #123` only when the change actually resolves the issue; use a reference when it addresses
only part of it. If you stop working on a change, leave a short handoff explaining what remains.
The [pull request guide](pull-requests.md) covers review and merge.
