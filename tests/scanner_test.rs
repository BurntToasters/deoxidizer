use deoxidizer_lib::config::{Config, Scope};
use deoxidizer_lib::scanner::{scan, scan_dir, ScanError};
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

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-tauri-app");
}

#[test]
fn test_scan_finds_all_rust_projects() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let projects = scan_dir(tmp.path(), &Scope::TauriAndRust).unwrap();
    assert_eq!(projects.len(), 2);
}

#[test]
fn test_scan_rust_only_excludes_tauri() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "my-tauri-app");
    create_mock_rust_project(tmp.path(), "my-rust-lib");

    let projects = scan_dir(tmp.path(), &Scope::RustOnly).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-rust-lib");
}

#[test]
fn test_scan_calculates_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "sized-app");

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
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
    let projects = scan_dir(tmp.path(), &Scope::TauriAndRust).unwrap();
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

    let projects = deoxidizer_lib::scanner::scan(&config).unwrap();
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

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
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

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "workspace-app");
    assert_eq!(projects[0].path, workspace.canonicalize().unwrap());
    assert_eq!(
        projects[0].artifact_dir,
        workspace.join("target").canonicalize().unwrap()
    );
}

#[test]
fn test_scan_detects_inherited_renamed_tauri_dependency() {
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
framework = { package = "tauri", version = "2" }
"#,
    )
    .unwrap();
    fs::write(
        member.join("Cargo.toml"),
        r#"[package]
name = "workspace-renamed"
version = "0.1.0"
edition = "2021"

[dependencies]
framework.workspace = true
"#,
    )
    .unwrap();
    fs::write(workspace.join("target/debug/app"), b"artifact").unwrap();

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "workspace-renamed");
}

#[test]
fn test_scan_invalid_root_returns_error() {
    let missing = tempfile::tempdir().unwrap().path().join("missing");
    assert!(scan_dir(&missing, &Scope::TauriAndRust).is_err());
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

    assert!(scan_dir(tmp.path(), &Scope::TauriAndRust)
        .unwrap()
        .is_empty());
}

#[test]
fn test_scan_respects_min_size_mb() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "tiny-app");

    let filtered = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriOnly,
        min_size_mb: 1,
        ..Default::default()
    };
    assert!(scan(&filtered).unwrap().is_empty());

    let retained = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriOnly,
        min_size_mb: 0,
        ..Default::default()
    };
    let projects = scan(&retained).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "tiny-app");
}

#[test]
fn test_scan_ignored_projects_is_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "MyTauriApp");

    let config = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        ignored_projects: vec!["mytauriapp".to_string()],
        ..Default::default()
    };
    assert!(scan(&config).unwrap().is_empty());

    let config_upper = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        ignored_projects: vec!["MYTAURIAPP".to_string()],
        ..Default::default()
    };
    assert!(scan(&config_upper).unwrap().is_empty());
}

#[test]
fn test_scan_skips_hidden_and_artifact_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "visible-app");
    create_mock_tauri_project(&tmp.path().join(".hidden"), "hidden-app");
    create_mock_tauri_project(&tmp.path().join(".git"), "git-app");
    create_mock_tauri_project(&tmp.path().join("node_modules"), "node-app");

    let projects = scan_dir(tmp.path(), &Scope::TauriAndRust).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "visible-app");
}

#[test]
fn test_scan_triple_artifact_breakdown() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "triple-app");
    let target = tmp
        .path()
        .join("triple-app")
        .join("src-tauri")
        .join("target");

    let triple_debug_deps = target.join("aarch64-apple-darwin").join("debug/deps");
    let triple_debug_inc = target
        .join("aarch64-apple-darwin")
        .join("debug/incremental");
    let triple_release = target.join("x86_64-pc-windows-msvc").join("release");
    fs::create_dir_all(&triple_debug_deps).unwrap();
    fs::create_dir_all(&triple_debug_inc).unwrap();
    fs::create_dir_all(&triple_release).unwrap();
    fs::write(triple_debug_deps.join("lib.rlib"), vec![0u8; 2048]).unwrap();
    fs::write(triple_debug_inc.join("cache.incr"), vec![0u8; 256]).unwrap();
    fs::write(triple_release.join("app.exe"), vec![0u8; 4096]).unwrap();

    let projects = scan_dir(tmp.path(), &Scope::TauriOnly).unwrap();
    assert_eq!(projects.len(), 1);
    let breakdown = projects[0].breakdown.as_ref().expect("breakdown");
    assert_eq!(breakdown.debug_size, 1024 + 512 + 2048 + 256);
    assert_eq!(breakdown.release_size, 4096);
    assert_eq!(breakdown.incremental_size, 512 + 256);
    assert_eq!(breakdown.deps_size, 1024 + 2048);
}

/// Backdate a fixture file by whole days (wide margins only, never exact-now).
fn backdate_file(path: &Path, days_ago: u64) {
    #[cfg(unix)]
    {
        // POSIX touch -t works on both GNU and BSD; compute the stamp with
        // whichever date dialect the platform provides.
        let stamp = std::process::Command::new("date")
            .arg(format!("-v-{days_ago}d"))
            .arg("+%Y%m%d%H%M.%S")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .or_else(|| {
                std::process::Command::new("date")
                    .arg("-d")
                    .arg(format!("{days_ago} days ago"))
                    .arg("+%Y%m%d%H%M.%S")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            })
            .expect("date must be available to backdate fixtures");
        let status = std::process::Command::new("touch")
            .arg("-t")
            .arg(&stamp)
            .arg(path)
            .status()
            .expect("touch must be available to backdate fixtures");
        assert!(status.success(), "touch -t failed for {}", path.display());
    }
    #[cfg(windows)]
    {
        let literal = path.to_string_lossy().replace('\'', "''");
        let script = format!(
            "(Get-Item -LiteralPath '{literal}').LastWriteTime = (Get-Date).AddDays(-{days_ago})"
        );
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .expect("powershell must be available to backdate fixtures");
        assert!(status.success());
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, days_ago);
        panic!("backdating fixtures is only supported on unix/windows");
    }
}

#[test]
fn test_older_than_retains_stale_drops_fresh_and_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_tauri_project(tmp.path(), "stale-app");
    create_mock_tauri_project(tmp.path(), "fresh-app");
    // Project with an empty target/: no file mtimes means unknown age.
    let unknown = tmp.path().join("unknown-app");
    fs::create_dir_all(unknown.join("target")).unwrap();
    fs::write(
        unknown.join("Cargo.toml"),
        "[package]\nname = \"unknown-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntauri = \"2\"\n",
    )
    .unwrap();

    // Wide margins only: 5-day-old fixtures against a 2-day cutoff.
    for file in ["debug/deps/dummy.rlib", "debug/incremental/dummy.incr"] {
        backdate_file(&tmp.path().join("stale-app/src-tauri/target").join(file), 5);
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deox"))
        .args([
            "scan",
            "--path",
            tmp.path().to_str().unwrap(),
            "--older-than",
            "2",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("stale-app"),
        "stale must be retained:\n{stdout}"
    );
    assert!(
        !stdout.contains("fresh-app"),
        "fresh must be dropped:\n{stdout}"
    );
    assert!(
        !stdout.contains("unknown-app"),
        "unknown age must be dropped:\n{stdout}"
    );
}

#[test]
fn test_scan_min_size_boundary_and_overflow() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("exact-app");
    fs::create_dir_all(project.join("target/debug")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"exact-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntauri = \"2\"\n",
    )
    .unwrap();
    // Exactly 1 MiB of logical artifact bytes.
    fs::write(
        project.join("target/debug/blob.bin"),
        vec![0u8; 1024 * 1024],
    )
    .unwrap();

    let at_limit = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        min_size_mb: 1,
        ..Default::default()
    };
    // Boundary is inclusive (>=): exactly 1 MiB is retained.
    assert_eq!(scan(&at_limit).unwrap().len(), 1);

    let above = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        min_size_mb: 2,
        ..Default::default()
    };
    assert!(scan(&above).unwrap().is_empty());

    // Overflow must yield empty results, never panic.
    let overflow = Config {
        projects_dir: tmp.path().to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        min_size_mb: u64::MAX,
        ..Default::default()
    };
    assert!(scan(&overflow).unwrap().is_empty());
}

#[test]
fn test_scan_sort_ties_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    create_mock_rust_project(tmp.path(), "aaa-app");
    create_mock_rust_project(tmp.path(), "zzz-app");

    let first = scan_dir(tmp.path(), &Scope::TauriAndRust).unwrap();
    let second = scan_dir(tmp.path(), &Scope::TauriAndRust).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].artifact_size, first[1].artifact_size);
    // Sorted by size descending at minimum.
    assert!(first[0].artifact_size >= first[1].artifact_size);
    // Identical order across repeated scans.
    let first_names: Vec<&str> = first.iter().map(|project| project.name.as_str()).collect();
    let second_names: Vec<&str> = second.iter().map(|project| project.name.as_str()).collect();
    assert_eq!(first_names, second_names);
}

#[cfg(unix)]
#[test]
fn test_scan_permission_denied_no_panic() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    create_mock_rust_project(tmp.path(), "visible-app");
    let locked = tmp.path().join("locked");
    fs::create_dir_all(locked.join("target/debug")).unwrap();
    fs::write(
        locked.join("Cargo.toml"),
        "[package]\nname = \"locked\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(locked.join("target/debug/blob"), b"artifact").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Permissions are not enforced (e.g. running as root): premise void.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let result = scan_dir(tmp.path(), &Scope::TauriAndRust);
    let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
    match result {
        Ok(_) => {}
        Err(ScanError::Traversal(_)) => {}
        Err(other) => panic!("expected Ok or Traversal, got {other}"),
    }
}
