# rivium-test

Test support for services built on Rivium: scripted services, log capture, process-level test tools and lifecycle contract suites. Use it as a dev-dependency only.

**Status:** pre-release, not published yet. Implemented so far: `ScriptedService`, which follows
a list of steps to drive the supervisor through any order of events; `capture_logs`, which
captures a test's log events as JSON objects; `process`, which runs a service program as a
supervisor would (signals, events, exit codes) and checks the lifecycle contract every program
keeps; and `deps::assert_closure_excludes`, which checks that a package's dependency closure on
the test's target platform contains no banned crate. The embedded contract suite comes with the
embedded host. See the [repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
