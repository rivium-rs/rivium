# rivium-jni

The JNI adapter for Rivium's embedded host: a macro that generates `JNI_OnLoad` and registers a fixed set of native methods, so the application's cdylib can keep `#![forbid(unsafe_code)]`.

```rust
#![forbid(unsafe_code)]

rivium_jni::export!(class = "com/example/svc/RiviumBridge", app = my_svc::App);
```

The generated `JNI_OnLoad`, an `unsafe` function that only the JVM calls, registers
`nativeStart`, `nativeStop`, `nativeStatus`, `nativeLastError` and `nativeVersion` on the bridge
class, which drive the application's embedded host; a class that declares them otherwise fails
to load. A panic in a native method returns -99 or an empty string, and the crate refuses
`panic = "abort"`. The repository's [bridge declaration](https://github.com/rivium-rs/rivium/blob/main/crates/rivium-jni/tests/jvm/RiviumBridge.java)
is the reference for Java callers. The [desktop JVM checks](https://github.com/rivium-rs/rivium/blob/main/docs/development.md#jvm-contracts)
cover loading and the native contract; their fixtures and runner are repository-only and are
not included in the published crate.

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
