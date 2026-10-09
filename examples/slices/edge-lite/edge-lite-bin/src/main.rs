//! The `edge-lite` program: the edge-lite service library on the process host.

fn main() -> std::process::ExitCode {
    rivium::process::run::<edge_lite::EdgeLite>()
}
