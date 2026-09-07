use deoxidizer_lib::config::Scope;
use deoxidizer_lib::scanner::scan_dir;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::symlink;

/// Create a minimal mock Tauri project structure.
fn create_mock_tauri_project(root: &Path, name: &str) {
    let project_dir = root.join(name).join("src-tauri");
    let target_dir = project_dir.join("target");
    let debug_dir = target_dir.join("debug").join("deps");
    let incremental_dir = target_dir.join("debug").join("incremental");

    fs::create_dir_all(&debug_dir).unwrap();
    fs::create_dir_all(&incremental_dir).unwrap();

    // Write a minimal Cargo.toml with tauri dependency
    fs::write(
        project_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = "2"
"#
        ),
    )
    .unwrap();

    // Create some dummy files to give the target/ directory size
    fs::write(debug_dir.join("dummy.rlib"), vec![0u8; 1024]).unwrap();
    fs::write(incremental_dir.join("dummy.incr"), vec![0u8; 512]).unwrap();
}

/// Create a minimal mock plain Rust project.
fn create_mock_rust_project(root: &Path, name: &str) {
    let project_dir = root.join(name);
    let target_dir = project_dir.join("target");
    let debug_dir = target_dir.join("debug");

    fs::create_dir_all(&debug_dir).unwrap();

    fs::write(
        project_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1"
"#
        ),
    )
    .unwrap();

    fs::write(debug_dir.join("dummy.rlib"), vec![0u8; 2048]).unwrap();
}

#[test]
fn test_scan_finds_tauri_projects() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-tauri-app");
}

#[test]
fn test_scan_finds_all_rust_projects() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let projects = scan_dir(tmp.path(), &Scope::TauriAndRust);
    assert_eq!(projects.len(), 2);
}

#[test]
fn test_scan_rust_only_excludes_tauri() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let projects = scan_dir(tmp.path(), &Scope::RustOnly);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-rust-lib");
}

#[test]
fn test_scan_calculates_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "sized-app");

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly);
    assert_eq!(projects.len(), 1);
    assert!(projects[0].artifact_size > 0);

    if let Some(ref breakdown) = projects[0].breakdown {
        assert!(breakdown.debug_size > 0);
    } else {
        panic!("Expected breakdown to be present");
    }
}

#[test]
fn test_scan_empty_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = scan_dir(tmp.path(), &Scope::TauriAndRust);
    assert!(projects.is_empty());
}

#[test]
fn test_scan_respects_ignored_projects() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let config = deoxidizer_lib::config::Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        ignored_projects: vec!["my-tauri-app".to_string()],
        ..Default::default()
    };

    let projects = deoxidizer_lib::scanner::scan(&config);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-rust-lib");
}

#[test]
fn test_scan_detects_renamed_tauri_dependency() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("renamed-tauri");
    fs::create_dir_all(project.join("target/debug")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        r#"[package]
name = "renamed-tauri"
version = "0.1.0"
edition = "2021"

[dependencies]
framework = { package = "tauri", version = "2" }
"#,
    )
    .unwrap();
    fs::write(project.join("target/debug/app"), b"artifact").unwrap();

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "renamed-tauri");
}

#[test]
fn test_scan_detects_workspace_shared_target() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let member = workspace.join("app");
    fs::create_dir_all(workspace.join("target/debug")).unwrap();
    fs::create_dir_all(&member).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        r#"[workspace]
members = ["app"]

[workspace.dependencies]
tauri = "2"
"#,
    )
    .unwrap();
    fs::write(
        member.join("Cargo.toml"),
        r#"[package]
name = "workspace-app"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri.workspace = true
"#,
    )
    .unwrap();
    fs::write(workspace.join("target/debug/app"), b"artifact").unwrap();

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "workspace-app");
    assert_eq!(projects[0].path, workspace.canonicalize().unwrap());
    assert_eq!(
        projects[0].artifact_dir,
        workspace.join("target").canonicalize().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn test_scan_ignores_symlinked_target() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(outside.path().join("target/debug")).unwrap();
    fs::write(outside.path().join("target/debug/artifact"), b"outside").unwrap();

    let project = tmp.path().join("symlinked");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        r#"[package]
name = "symlinked"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    symlink(outside.path().join("target"), project.join("target")).unwrap();

    assert!(scan_dir(tmp.path(), &Scope::TauriAndRust).is_empty());
}
