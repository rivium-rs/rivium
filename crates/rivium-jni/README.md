# rivium-jni

The JNI adapter for Rivium's embedded host: a macro that generates `JNI_OnLoad` and registers a fixed set of native methods, so the application's cdylib can keep `#![forbid(unsafe_code)]`.

**Status:** pre-release skeleton. Nothing is implemented or published yet; see the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
