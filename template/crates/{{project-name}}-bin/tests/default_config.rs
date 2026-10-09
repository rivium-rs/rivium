//! configs/default.toml, which ships with the program, is what `{{project-name}} default-config`
//! prints: every key with its default. `just default-config` rewrites it.
#![cfg(not(target_os = "android"))]

use std::path::Path;

#[test]
fn configs_default_toml_is_the_default_configuration() {
    let bin = Path::new(env!("CARGO_BIN_EXE_{{project-name}}"));
    let output = rivium_test::process::command(bin)
        .arg("default-config")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "default-config: {stderr}");
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/default.toml");
    let shipped = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\r\n", "\n");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed == shipped,
        "{} is not the default configuration: run `just default-config`\n{printed}",
        file.display()
    );
}
