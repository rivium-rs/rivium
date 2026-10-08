# rivium-jni

The JNI adapter for Rivium's embedded host: a macro that generates `JNI_OnLoad` and registers a fixed set of native methods, so the application's cdylib can keep `#![forbid(unsafe_code)]`.

```rust
#![forbid(unsafe_code)]

rivium_jni::export!(class = "com/example/svc/RiviumBridge", app = my_svc::App);
```

The generated `JNI_OnLoad` registers `nativeStart`, `nativeStop`, `nativeStatus`,
`nativeLastError` and `nativeVersion` on the bridge class, which drive the application's
embedded host; a class that declares them otherwise fails to load. A panic in a native method
returns -99 or an empty string, and the crate refuses `panic = "abort"`.
`tests/jvm/RiviumBridge.java` is the reference declaration, and `tests/jvm/run.sh` checks a
library on a desktop JVM.

**Status:** pre-release, not published yet. See the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
