# rivium-test

Test support for services built on Rivium: scripted services, log capture, process-level test tools and lifecycle contract suites. Use it as a dev-dependency only.

**Status:** pre-release, not published yet. Implemented so far: `ScriptedService`, which follows
a list of steps to drive the supervisor through any order of events; `capture_logs`, which
captures a test's log events as JSON objects; and `deps::assert_closure_excludes`, which checks
that a package's dependency closure on the test's target platform contains no banned crate. The
process-level tools and the lifecycle contract suites come next. See the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
