use rivium_host::{Action, Options};
use std::ffi::OsString;
#[test]
fn cli_commands_relative_paths_and_embedded_inputs_are_explicit() {
    let current = std::env::current_dir().unwrap();
    for (command, action) in [
        ("run", Action::Run),
        ("check-config", Action::CheckConfig),
        ("default-config", Action::DefaultConfig),
        ("--version", Action::Version),
        ("-V", Action::Version),
        ("--help", Action::Help),
        ("-h", Action::Help),
    ] {
        let options = Options::parse(
            "options-test",
            [command, "--root", "relative", "--config", "file.toml"].map(OsString::from),
            false,
        )
        .unwrap();
        assert_eq!(options.action, action);
        assert_eq!(options.paths.root(), current.join("relative"));
        assert_eq!(
            options.paths.config_file(),
            current.join("relative/file.toml")
        );
        assert!(options.environment.is_empty());
    }
    assert_eq!(std::env::current_dir().unwrap(), current);
    for arguments in [
        vec!["--unknown"],
        vec!["run", "--version"],
        vec!["--root"],
        vec!["--config"],
        vec!["--set"],
    ] {
        assert!(
            Options::parse(
                "options-test",
                arguments.into_iter().map(OsString::from),
                false
            )
            .is_err()
        );
    }
    assert!(Options::parse("options-test", [], false).is_err());
}
#[cfg(unix)]
#[test]
fn paths_need_not_be_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let root = OsString::from_vec(b"/srv/\xff".to_vec());
    let options = Options::parse(
        "options-test",
        [OsString::from("--root"), root.clone()],
        false,
    )
    .unwrap();
    assert_eq!(options.paths.root().as_os_str(), root);
}
#[cfg(unix)]
#[test]
fn configuration_environment_rejects_non_utf8_values() {
    use std::os::unix::ffi::OsStringExt;
    if std::env::var_os("RIVIUM_OPTIONS_INVALID_ENV_CHILD").is_some() {
        let error = Options::parse("utf8-check", [], true).unwrap_err();
        assert!(error.to_string().contains("UTF-8"));
        return;
    }
    let output = rivium_test::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "configuration_environment_rejects_non_utf8_values",
            "--nocapture",
        ])
        .env("RIVIUM_OPTIONS_INVALID_ENV_CHILD", "1")
        .env("UTF8_CHECK_VALUE", OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
