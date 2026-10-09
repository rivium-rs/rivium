//! The `{{project-name}}` program: the service library on Rivium's process host.

fn main() -> std::process::ExitCode {
    rivium::process::run::<{{crate_name}}::{{project-name | pascal_case}}>()
}
