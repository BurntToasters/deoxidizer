use std::path::Path;
use std::process::Command;

fn apply_home_env(command: &mut Command, home: &Path) {
    #[cfg(windows)]
    command.env("USERPROFILE", home).env_remove("HOME");
    #[cfg(not(windows))]
    command.env("HOME", home);
}

fn write_test_config(home: &Path, projects_dir: &Path) {
    // Forward slashes parse as the same path on every platform and avoid
    // JSON escaping issues with Windows separators.
    let projects = projects_dir.to_string_lossy().replace('\\', "/");
    let content = format!(
        "{{\"version\": 2, \"projects_dir\": \"{projects}\", \"scope\": \"tauri-and-rust\", \
         \"clean_behavior\": \"delete\", \"default_mode\": \"full\", \"min_size_mb\": 0, \
         \"ignored_projects\": []}}"
    );
    std::fs::write(home.join(".deox_config"), content).unwrap();
}

fn create_clean_project(projects_dir: &Path) -> std::path::PathBuf {
    let root = projects_dir.join("test-project");
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"test-project\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntauri = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("target/debug/blob.bin"), vec![0u8; 1024]).unwrap();
    root
}

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
fn scan_invalid_root_exits_nonzero() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["scan", "--path", missing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Scan failed"));
}

#[test]
fn malformed_config_does_not_fall_back_to_destructive_defaults() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".deox_config"), "{not-json").unwrap();
    let projects = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    #[cfg(windows)]
    command.env("USERPROFILE", home.path()).env_remove("HOME");
    #[cfg(not(windows))]
    command.env("HOME", home.path());
    let output = command
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command
        .args(["settings", "config", "--scope", "invalid"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!home.path().join(".deox_config").exists());
}

#[test]
fn scan_help_output_parity_between_binaries() {
    let deoxidizer = Command::new(env!("CARGO_BIN_EXE_deoxidizer"))
        .args(["scan", "--help"])
        .output()
        .unwrap();
    let deox = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["scan", "--help"])
        .output()
        .unwrap();

    assert!(deoxidizer.status.success());
    assert!(deox.status.success());
    assert_eq!(deoxidizer.status.code(), deox.status.code());
    let deoxidizer_stdout = String::from_utf8_lossy(&deoxidizer.stdout);
    let deox_stdout = String::from_utf8_lossy(&deox.stdout);
    for stdout in [&deoxidizer_stdout, &deox_stdout] {
        assert!(stdout.contains("--path"));
        assert!(stdout.contains("--min-size"));
    }
    // Usage line embeds the invoked binary name, so normalize before comparing.
    let normalized = deoxidizer_stdout.replace("deoxidizer", "deox");
    assert_eq!(normalized, deox_stdout);
}

#[test]
fn inspect_missing_path_exit_codes_match_between_binaries() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("does-not-exist-12345");
    assert!(!missing.exists());

    for binary in [env!("CARGO_BIN_EXE_deoxidizer"), env!("CARGO_BIN_EXE_deox")] {
        let output = Command::new(binary)
            .args(["inspect", missing.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(!output.status.success(), "binary: {binary}");
        assert_eq!(output.status.code(), Some(1), "binary: {binary}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Path does not exist"),
            "binary: {binary}"
        );
    }
}

#[test]
fn setup_default_writes_config_to_home() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command.args(["setup", "--default"]).output().unwrap();

    assert!(output.status.success());
    let config_path = home.path().join(".deox_config");
    assert!(config_path.exists());
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("\"version\": 2"));
}

#[test]
fn test_clean_dry_run_parity_between_binaries() {
    let home = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let root = create_clean_project(projects.path());
    write_test_config(home.path(), projects.path());
    let sentinel = root.join("target/debug/blob.bin");

    let mut runs = Vec::new();
    for binary in [env!("CARGO_BIN_EXE_deoxidizer"), env!("CARGO_BIN_EXE_deox")] {
        let mut command = Command::new(binary);
        apply_home_env(&mut command, home.path());
        let output = command
            .args([
                "clean",
                "--dry-run",
                "--yes",
                "--mode",
                "full",
                "--path",
                projects.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "binary: {binary}");
        runs.push(output);
    }

    // Dry run must leave the filesystem untouched for both invokers.
    assert!(sentinel.exists());
    assert!(root.join("target").exists());

    // Usage lines embed the invoked binary name, so normalize before comparing.
    let deoxidizer_stdout = String::from_utf8_lossy(&runs[0].stdout).replace("deoxidizer", "deox");
    let deox_stdout = String::from_utf8_lossy(&runs[1].stdout);
    assert_eq!(deoxidizer_stdout, deox_stdout);
    assert!(deox_stdout.contains("DRY RUN") || deox_stdout.contains("Would free"));
}

#[test]
fn test_clean_help_parity_between_binaries() {
    let deoxidizer = Command::new(env!("CARGO_BIN_EXE_deoxidizer"))
        .args(["clean", "--help"])
        .output()
        .unwrap();
    let deox = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["clean", "--help"])
        .output()
        .unwrap();

    assert!(deoxidizer.status.success());
    assert!(deox.status.success());
    let deoxidizer_stdout = String::from_utf8_lossy(&deoxidizer.stdout);
    let deox_stdout = String::from_utf8_lossy(&deox.stdout);
    for stdout in [&deoxidizer_stdout, &deox_stdout] {
        assert!(stdout.contains("--mode"));
        assert!(stdout.contains("--dry-run"));
    }
    let normalized = deoxidizer_stdout.replace("deoxidizer", "deox");
    assert_eq!(normalized, deox_stdout);
}

#[test]
fn test_inspect_help_parity_between_binaries() {
    let deoxidizer = Command::new(env!("CARGO_BIN_EXE_deoxidizer"))
        .args(["inspect", "--help"])
        .output()
        .unwrap();
    let deox = Command::new(env!("CARGO_BIN_EXE_deox"))
        .args(["inspect", "--help"])
        .output()
        .unwrap();

    assert!(deoxidizer.status.success());
    assert!(deox.status.success());
    let deoxidizer_stdout = String::from_utf8_lossy(&deoxidizer.stdout);
    let deox_stdout = String::from_utf8_lossy(&deox.stdout);
    for stdout in [&deoxidizer_stdout, &deox_stdout] {
        assert!(stdout.contains("--scope"));
    }
    let normalized = deoxidizer_stdout.replace("deoxidizer", "deox");
    assert_eq!(normalized, deox_stdout);
}

#[test]
fn test_invalid_settings_config_default_mode_exits_nonzero() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command
        .args(["settings", "config", "--default-mode", "invalid"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!home.path().join(".deox_config").exists());
}
