# {{project-name}}

A service built on [Rivium](https://github.com/rivium-rs/rivium){% if http %}, with an HTTP API{% endif %}{% if jni %}, which also runs
embedded in an Android app{% endif %}. AGENTS.md describes the layout and the rules; docs/architecture.md, the
service.

| Task | What it does |
| --- | --- |
| `just run`, `just check-config` | Runs the program with this directory as its root; checks and prints its configuration |
| `just check` | Formatting, clippy, cargo-deny, the panic check, the API documentation and the tests |
| `just fmt`, `just msrv`, `just audit` | Formats the code; builds with Rust 1.92; checks the security advisories |
| `just default-config` | Rewrites configs/default.toml with the program's defaults |
| `just update-rivium` | Takes Rivium's latest compatible release (its fixes) |{% if jni %}
| `just jvm` | The JNI library's contract on a desktop JVM (JDK 17) |{% endif %}
| `just cross`, `just package` | Builds the program for the platforms; archives for the installer |
| `just template-diff <version>` | The changes of the Rivium template since this project was generated |

Run the program from a directory of its own, which is its root: it reads
`configs/default.toml` there and writes its logs below `logs/{{project-name}}/`. `just run` uses
this directory as the root and passes its arguments on (`just run --set log.filter=debug`); the
program's options:

```bash
cargo run -p {{project-name}}-bin -- --help
```

Tools: Rust 1.92 or later, [just](https://github.com/casey/just) and
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny); cargo-zigbuild and zig, or cross,
to package for other platforms; cargo-generate for `template-diff`.
