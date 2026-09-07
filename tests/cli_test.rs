use std::process::Command;

#[test]
fn dual_binaries_report_identical_version() {
    let deoxidizer = Command::new(env!("CARGO_BIN_EXE_deoxidizer"))
        .arg("--version")
        .output()
        .unwrap();
    let deox = Command::new(env!("CARGO_BIN_EXE_deox"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(deoxidizer.status.success());
    assert!(deox.status.success());
    assert_eq!(deoxidizer.stdout, deox.stdout);
}

#[test]
fn invalid_clean_mode_fails_before_cleaning() {
    let output = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["clean", "--mode", "not-a-mode", "--yes"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value") || stderr.contains("possible values"));
}

#[test]
fn scan_path_is_read_only_and_aliases_share_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let deoxidizer = Command::new(env!("CARGO_BIN_EXE_deoxidizer"))
        .args(["scan", "--path", temp.path().to_str().unwrap()])
        .output()
        .unwrap();
    let deox = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["scan", "--path", temp.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(deoxidizer.status.success());
    assert!(deox.status.success());
    assert_eq!(deoxidizer.stdout, deox.stdout);
    assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
}

#[test]
fn malformed_config_does_not_fall_back_to_destructive_defaults() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".deox_config"), "{not-json").unwrap();
    let projects = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_deox"))
        .env("HOME", home.path())
        .args(["scan", "--path", projects.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Configuration error"));
    assert!(std::fs::read_dir(projects.path()).unwrap().next().is_none());
}

#[test]
fn invalid_settings_exit_nonzero_without_writing_config() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_deox"))
        .env("HOME", home.path())
        .args(["settings", "config", "--scope", "invalid"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!home.path().join(".deox_config").exists());
}
