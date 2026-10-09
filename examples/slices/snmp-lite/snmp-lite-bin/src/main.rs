//! The `snmp-lite` program: the snmp-lite service library on the process host.

fn main() -> std::process::ExitCode {
    rivium::process::run::<snmp_lite::SnmpLite>()
}
