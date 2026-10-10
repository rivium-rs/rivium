# rivium-test

Test support for services built on Rivium: scripted services, log capture, process-level test tools and lifecycle contract suites. Use it as a dev-dependency only.

- `process::lifecycle_contract(bin, args)` and `embedded::lifecycle_contract::<App>(root, args)`:
  the lifecycle contract that every program built on Rivium keeps, as a process and embedded,
  for a service's own tests;
- `process::spawn` and `process::command`: a program run as a supervisor runs it, with its events
  readable;
- `ScriptedService`, which follows a list of steps to drive the supervisor, and `capture_logs`;
- `deps::assert_closure_excludes`: the dependency closure of a package, on the test's platform,
  has none of the given crates.

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the examples and the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
