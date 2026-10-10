# rivium-error

The single error type of Rivium services: an error kind with a transport-independent class, the responsible party, a retry hint, context and a cause chain, plus structured log fields. It is part of every service's public API, so it stays small and depends only on `tracing`.

```rust
use rivium_error::prelude::*;

/// No device has the given serial number.
const DEVICE_NOT_FOUND: ErrorType = &ErrorKind::new("DeviceNotFound", Class::NotFound);

fn find(serial: &str) -> Result<()> {
    Error::e_explain(DEVICE_NOT_FOUND, format!("no device {serial}"))
}

fn main() {
    if let Err(error) = find("A-7") {
        log_error!(tracing::Level::WARN, &error, "lookup failed");
    }
}
```

`{}` shows an error's own context and `{:#}` the whole cause chain; `log_error!` writes the fields
`error.type`, `error.class`, `error.source`, `error.retry`, `error.context`, `error.chain` and
`error.cause`.

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the examples and the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
