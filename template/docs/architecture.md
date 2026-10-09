# {{project-name}}: architecture

<!-- Describe the service: what it does, for whom, and what it talks to. -->

## Services

The composition root, `{{project-name | pascal_case}}` in `crates/{{project-name}}/src/lib.rs`, builds them:

| Service | Kind | What it does | Configuration |
| --- | --- | --- | --- |{% if http %}
| `http` | frontline | The HTTP API (`src/api.rs`) | `[http]` |{% endif %}
| `heartbeat` | background | A sample job, to replace | `[heartbeat]` |

## Modules

<!-- The business modules of the service library: the domain, the application logic and the
adapters to the outside, and what each may use. -->

## Deployment

The program runs from its own directory: `configs/default.toml` next to it, logs below
`logs/{{project-name}}/`. `just package` builds the archives for: {{platforms}}.
