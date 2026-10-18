# rivium-jni

A thin export macro and normal Rust implementation for the embedded host:

```rust,ignore
rivium_jni::export!(class = "com/example/Bridge", app = Application);
```

The class must declare five static native methods:

```java
static native int nativeStart(String[] args);
static native int nativeStop(long timeoutMillis);
static native int nativeStatus();
static native String nativeLastError();
static native String nativeVersion();
```

One library exports one JNI_OnLoad. Rust wrapper ABIs are checked through the JNI crate;
Java declaration mismatch is checked at registration/loading, not by the Rust compiler.
Integer panics return -99 and string panics return an empty string. panic=abort is rejected.
A null args array means no arguments; a null element means an empty argument. Embedded start
requires --root. Negative stop durations clamp to zero. Unicode arguments and paths are supported.

Status values are Idle=0, Starting=1, Running=2, Stopping=3, Restarting=4, Failed=5.
Result codes include success=0, already-running=1, not-running=2, busy=3 and cancelled=4;
errors are negative process exit codes. Java last-error values are safe summaries, not raw
configuration diagnostics. Blocking start/stop calls belong off the Java UI thread.

Use only the embedded host dependency; no HTTP/process host is selected. Real desktop-JVM
contracts do not establish Android ART or device compatibility.
Requires Rust 1.92. Licensed under MIT or Apache-2.0.
