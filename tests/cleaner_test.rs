use deoxidizer_lib::cleaner::{clean_project, estimate_freed, CleanMode};
use deoxidizer_lib::config::{CleanBehavior, Config, Scope};
use deoxidizer_lib::project::{DiscoveredProject, ProjectKind, TargetBreakdown};
use std::fs;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::symlink;

/// Create a mock project with a populated target directory.
fn create_mock_project(root: &std::path::Path) -> DiscoveredProject {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    let target = root.join("target");
    let debug = target.join("debug");
    let release = target.join("release");
    let debug_deps = debug.join("deps");
    let debug_inc = debug.join("incremental");
    let release_deps = release.join("deps");

    fs::create_dir_all(&debug_deps).unwrap();
    fs::create_dir_all(&debug_inc).unwrap();
    fs::create_dir_all(&release_deps).unwrap();

    // Create files of known sizes
    fs::write(debug_deps.join("lib.rlib"), vec![0u8; 4096]).unwrap();
    fs::write(debug_inc.join("cache.incr"), vec![0u8; 2048]).unwrap();
    fs::write(release_deps.join("lib.rlib"), vec![0u8; 1024]).unwrap();
    fs::write(debug.join("binary"), vec![0u8; 512]).unwrap();

    DiscoveredProject {
        name: "test-project".to_string(),
        path: root.to_path_buf(),
        kind: ProjectKind::TauriApp,
        artifact_dir: target,
        artifact_size: 4096 + 2048 + 1024 + 512,
        last_modified: None,
        breakdown: Some(TargetBreakdown {
            debug_size: 4096 + 2048 + 512,
            release_size: 1024,
            incremental_size: 2048,
            deps_size: 4096 + 1024,
            other_size: 0,
        }),
    }
}

#[test]
fn test_estimate_freed_full() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let estimate = estimate_freed(&project, &CleanMode::Full).unwrap();
    assert!(estimate > 0);
}

#[test]
fn test_estimate_freed_debug_only() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let estimate = estimate_freed(&project, &CleanMode::DebugOnly).unwrap();
    assert!(estimate > 0);
    // Debug should be less than full (release still exists)
    let full_estimate = estimate_freed(&project, &CleanMode::Full).unwrap();
    assert!(estimate < full_estimate);
}

#[test]
fn test_clean_full_mode_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert!(bytes_freed > 0);
        }
        _ => panic!("Expected Cleaned result"),
    }

    // Verify target dir is gone
    assert!(!project.artifact_dir.exists());
}

#[test]
fn test_clean_debug_only_preserves_release() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(
        &project,
        &CleanMode::DebugOnly,
        &CleanBehavior::Delete,
        false,
    );
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert!(bytes_freed > 0);
        }
        _ => panic!("Expected Cleaned result"),
    }

    // debug/ should be gone, release/ should still exist
    assert!(!project.artifact_dir.join("debug").exists());
    assert!(project.artifact_dir.join("release").exists());
}

#[test]
fn test_clean_debug_only_handles_target_triple() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let cross_debug = project
        .artifact_dir
        .join("aarch64-unknown-linux-gnu")
        .join("debug");
    fs::create_dir_all(&cross_debug).unwrap();
    fs::write(cross_debug.join("binary"), vec![0u8; 128]).unwrap();

    let result = clean_project(
        &project,
        &CleanMode::DebugOnly,
        &CleanBehavior::Delete,
        false,
    );
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Cleaned { .. }
    ));
    assert!(!cross_debug.exists());
}

#[test]
fn test_clean_incremental_only() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(
        &project,
        &CleanMode::IncrementalOnly,
        &CleanBehavior::Delete,
        false,
    );
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert!(bytes_freed > 0);
        }
        _ => panic!("Expected Cleaned result"),
    }

    // incremental/ should be gone, deps/ should still exist
    assert!(!project
        .artifact_dir
        .join("debug")
        .join("incremental")
        .exists());
    assert!(project.artifact_dir.join("debug").join("deps").exists());
}

#[test]
fn test_dry_run_does_not_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, true);
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert!(bytes_freed > 0);
        }
        _ => panic!("Expected Cleaned result"),
    }

    // Target should still exist after dry run
    assert!(project.artifact_dir.exists());
}

#[test]
fn test_clean_deps_only_preserves_incremental() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(
        &project,
        &CleanMode::DepsOnly,
        &CleanBehavior::Delete,
        false,
    );
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert!(bytes_freed > 0);
        }
        other => panic!("Expected Cleaned result, got {other:?}"),
    }

    assert!(!project.artifact_dir.join("debug/deps").exists());
    assert!(!project.artifact_dir.join("release/deps").exists());
    assert!(project.artifact_dir.join("debug/incremental").exists());
}

#[cfg(unix)]
#[test]
fn test_clean_rejects_symlinked_target() {
    let project_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    let outside_target = outside_root.path().join("target");
    fs::create_dir_all(&outside_target).unwrap();
    let sentinel = outside_target.join("sentinel");
    fs::write(&sentinel, b"must survive").unwrap();

    let target = project_root.path().join("target");
    fs::write(
        project_root.path().join("Cargo.toml"),
        "[package]\nname = \"symlinked\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    symlink(&outside_target, &target).unwrap();
    let project = DiscoveredProject {
        name: "symlinked".to_string(),
        path: project_root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: target,
        artifact_size: 1,
        last_modified: None,
        breakdown: None,
    };

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Error { .. }
    ));
    assert!(sentinel.exists());
}

#[cfg(unix)]
#[test]
fn test_clean_rejects_symlink_inside_target() {
    let project_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    let outside_file = outside_root.path().join("sentinel");
    fs::write(&outside_file, b"must survive").unwrap();

    let target = project_root.path().join("target/debug");
    fs::write(
        project_root.path().join("Cargo.toml"),
        "[package]\nname = \"nested-symlink\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(&target).unwrap();
    symlink(&outside_file, target.join("linked-file")).unwrap();
    let project = DiscoveredProject {
        name: "nested-symlink".to_string(),
        path: project_root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: project_root.path().join("target"),
        artifact_size: 1,
        last_modified: None,
        breakdown: None,
    };

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Error { .. }
    ));
    assert!(outside_file.exists());
}

#[test]
fn test_clean_rejects_forged_project_identity() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target/debug");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"real-project\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(target.join("artifact"), b"must survive").unwrap();
    let project = DiscoveredProject {
        name: "forged-project".to_string(),
        path: root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: root.path().join("target"),
        artifact_size: 1,
        last_modified: None,
        breakdown: None,
    };
    assert!(matches!(
        clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false),
        deoxidizer_lib::cleaner::CleanResult::Error { .. }
    ));
    assert!(target.join("artifact").exists());
}

#[test]
fn test_estimate_freed_incremental_and_deps_match_breakdown() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let breakdown = project.breakdown.as_ref().expect("breakdown");
    assert_eq!(breakdown.incremental_size, 2048);
    assert_eq!(breakdown.deps_size, 4096 + 1024);
    assert_eq!(
        estimate_freed(&project, &CleanMode::IncrementalOnly).unwrap(),
        breakdown.incremental_size
    );
    assert_eq!(
        estimate_freed(&project, &CleanMode::DepsOnly).unwrap(),
        breakdown.deps_size
    );
}

#[test]
fn test_clean_triple_incremental_selective() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let triple_inc = project
        .artifact_dir
        .join("aarch64-apple-darwin")
        .join("debug/incremental");
    let triple_deps = project
        .artifact_dir
        .join("aarch64-apple-darwin")
        .join("debug/deps");
    fs::create_dir_all(&triple_inc).unwrap();
    fs::create_dir_all(&triple_deps).unwrap();
    fs::write(triple_inc.join("cache.incr"), vec![0u8; 64]).unwrap();
    fs::write(triple_deps.join("lib.rlib"), vec![0u8; 64]).unwrap();

    let result = clean_project(
        &project,
        &CleanMode::IncrementalOnly,
        &CleanBehavior::Delete,
        false,
    );
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Cleaned { .. }
    ));
    assert!(!triple_inc.exists());
    assert!(!project.artifact_dir.join("debug/incremental").exists());
    assert!(triple_deps.exists());
    assert!(project.artifact_dir.join("debug/deps").exists());
}

#[test]
fn test_clean_triple_deps_selective() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let triple_inc = project
        .artifact_dir
        .join("x86_64-pc-windows-msvc")
        .join("release/incremental");
    let triple_deps = project
        .artifact_dir
        .join("x86_64-pc-windows-msvc")
        .join("release/deps");
    fs::create_dir_all(&triple_inc).unwrap();
    fs::create_dir_all(&triple_deps).unwrap();
    fs::write(triple_inc.join("cache.incr"), vec![0u8; 64]).unwrap();
    fs::write(triple_deps.join("lib.rlib"), vec![0u8; 64]).unwrap();

    let result = clean_project(
        &project,
        &CleanMode::DepsOnly,
        &CleanBehavior::Delete,
        false,
    );
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Cleaned { .. }
    ));
    assert!(!triple_deps.exists());
    assert!(!project.artifact_dir.join("debug/deps").exists());
    assert!(triple_inc.exists());
    assert!(project.artifact_dir.join("debug/incremental").exists());
}

#[test]
fn test_clean_skipped_when_no_matching_dirs() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"empty-target\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("target")).unwrap();
    let project = DiscoveredProject {
        name: "empty-target".to_string(),
        path: root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: root.path().join("target"),
        artifact_size: 0,
        last_modified: None,
        breakdown: None,
    };
    let result = clean_project(
        &project,
        &CleanMode::IncrementalOnly,
        &CleanBehavior::Delete,
        false,
    );
    assert!(matches!(
        result,
        deoxidizer_lib::cleaner::CleanResult::Skipped { .. }
    ));
    if let deoxidizer_lib::cleaner::CleanResult::Skipped { reason } = result {
        assert!(!reason.is_empty());
    }

    let release_only = tempfile::tempdir().unwrap();
    fs::write(
        release_only.path().join("Cargo.toml"),
        "[package]\nname = \"release-only\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let release_dir = release_only.path().join("target/release");
    fs::create_dir_all(&release_dir).unwrap();
    fs::write(release_dir.join("app"), b"artifact").unwrap();
    let release_project = DiscoveredProject {
        name: "release-only".to_string(),
        path: release_only.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: release_only.path().join("target"),
        artifact_size: 8,
        last_modified: None,
        breakdown: None,
    };
    assert!(matches!(
        clean_project(
            &release_project,
            &CleanMode::DebugOnly,
            &CleanBehavior::Delete,
            false
        ),
        deoxidizer_lib::cleaner::CleanResult::Skipped { .. }
    ));
}

#[test]
fn test_clean_result_partial_and_skipped_shapes() {
    let partial = deoxidizer_lib::cleaner::CleanResult::Partial {
        bytes_freed: 42,
        message: "one path failed".to_string(),
    };
    match partial {
        deoxidizer_lib::cleaner::CleanResult::Partial {
            bytes_freed,
            message,
        } => {
            assert_eq!(bytes_freed, 42);
            assert!(!message.is_empty());
        }
        other => panic!("expected Partial, got {other:?}"),
    }
    let skipped = deoxidizer_lib::cleaner::CleanResult::Skipped {
        reason: "No matching directories found".to_string(),
    };
    match skipped {
        deoxidizer_lib::cleaner::CleanResult::Skipped { reason } => {
            assert!(!reason.is_empty());
        }
        other => panic!("expected Skipped, got {other:?}"),
    }
}

/// Intentionally weak on headless CI where no Trash exists: accepts Cleaned or
/// a trash-containing Error so the suite stays green without a desktop session.
/// Set DEOX_STRICT_TRASH=1 for strict local runs with Trash available, which
/// require Cleaned success.
#[test]
fn test_clean_full_trash_moves_target() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Trash, false);
    match result {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { .. } => {
            assert!(!project.artifact_dir.exists());
        }
        deoxidizer_lib::cleaner::CleanResult::Error { message } => {
            if std::env::var("DEOX_STRICT_TRASH").as_deref() == Ok("1") {
                panic!("DEOX_STRICT_TRASH=1 requires Trash success, got error: {message}");
            }
            assert!(
                message.to_lowercase().contains("trash"),
                "trash unsupported, expected trash error, got: {message}"
            );
        }
        other => panic!("expected Cleaned or trash Error, got {other:?}"),
    }
}

fn write_cli_test_config(home: &std::path::Path, projects_dir: &std::path::Path) {
    let config = Config {
        projects_dir: projects_dir.to_string_lossy().to_string(),
        scope: Scope::TauriAndRust,
        ..Default::default()
    };
    config
        .save_to(&home.join(".deox_config"))
        .expect("write test config");
}

fn apply_home_env(command: &mut Command, home: &std::path::Path) {
    #[cfg(windows)]
    command.env("USERPROFILE", home).env_remove("HOME");
    #[cfg(not(windows))]
    command.env("HOME", home);
}

fn create_cli_project(projects_dir: &std::path::Path) -> std::path::PathBuf {
    let root = projects_dir.join("test-project");
    fs::create_dir_all(&root).unwrap();
    create_mock_project(&root);
    root
}

#[test]
fn test_cli_clean_dry_run_is_read_only() {
    let home = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let root = create_cli_project(projects.path());
    write_cli_test_config(home.path(), projects.path());
    let sentinel = root.join("target/debug/deps/lib.rlib");
    assert!(sentinel.exists());

    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command
        .args(["clean", "--dry-run", "--yes", "--mode", "full"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(sentinel.exists());
    assert!(root.join("target").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("DRY RUN") || stdout.contains("Would free"));
}

#[test]
fn test_cli_clean_older_than_filters_fresh_artifacts() {
    let home = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let root = create_cli_project(projects.path());
    write_cli_test_config(home.path(), projects.path());

    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command
        .args([
            "clean",
            "--dry-run",
            "--yes",
            "--older-than",
            "30",
            "--mode",
            "full",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(root.join("target").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No projects"));
}

#[test]
fn test_cli_clean_path_override_is_read_only() {
    let home = tempfile::tempdir().unwrap();
    let configured = tempfile::tempdir().unwrap();
    let override_dir = tempfile::tempdir().unwrap();
    let root = create_cli_project(override_dir.path());
    write_cli_test_config(home.path(), configured.path());

    let mut command = Command::new(env!("CARGO_BIN_EXE_deox"));
    apply_home_env(&mut command, home.path());
    let output = command
        .args([
            "clean",
            "--dry-run",
            "--yes",
            "--path",
            override_dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(root.join("target").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test-project"));
}

#[test]
fn test_dry_run_selective_modes_are_read_only() {
    for mode in [
        CleanMode::DebugOnly,
        CleanMode::IncrementalOnly,
        CleanMode::DepsOnly,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let project = create_mock_project(tmp.path());

        let result = clean_project(&project, &mode, &CleanBehavior::Delete, true);
        match result {
            deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
                assert!(bytes_freed > 0, "dry-run bytes for {mode}");
            }
            other => panic!("expected Cleaned dry-run for {mode}, got {other:?}"),
        }

        // Filesystem must be untouched by any dry run.
        assert!(project.artifact_dir.join("debug/deps/lib.rlib").exists());
        assert!(project
            .artifact_dir
            .join("debug/incremental/cache.incr")
            .exists());
        assert!(project.artifact_dir.join("release/deps/lib.rlib").exists());
        assert!(project.artifact_dir.join("debug/binary").exists());
    }
}

/// Intentionally weak on headless CI where no Trash exists: accepts Cleaned or
/// a trash-containing Error so the suite stays green without a desktop session.
/// Set DEOX_STRICT_TRASH=1 for strict local runs with Trash available, which
/// require Cleaned success.
#[test]
fn test_trash_selective_modes_move_or_error() {
    for mode in [
        CleanMode::DebugOnly,
        CleanMode::IncrementalOnly,
        CleanMode::DepsOnly,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let project = create_mock_project(tmp.path());

        let result = clean_project(&project, &mode, &CleanBehavior::Trash, false);
        match result {
            deoxidizer_lib::cleaner::CleanResult::Cleaned { .. } => match mode {
                CleanMode::DebugOnly => {
                    assert!(!project.artifact_dir.join("debug").exists());
                    assert!(project.artifact_dir.join("release").exists());
                }
                CleanMode::IncrementalOnly => {
                    assert!(!project.artifact_dir.join("debug/incremental").exists());
                    assert!(project.artifact_dir.join("debug/deps").exists());
                }
                _ => {
                    assert!(!project.artifact_dir.join("debug/deps").exists());
                    assert!(project.artifact_dir.join("debug/incremental").exists());
                }
            },
            deoxidizer_lib::cleaner::CleanResult::Error { message } => {
                if std::env::var("DEOX_STRICT_TRASH").as_deref() == Ok("1") {
                    panic!(
                        "DEOX_STRICT_TRASH=1 requires Trash success for {mode}, got error: {message}"
                    );
                }
                assert!(
                    message.to_lowercase().contains("trash"),
                    "trash unsupported, expected trash error for {mode}, got: {message}"
                );
            }
            other => panic!("expected Cleaned or trash Error for {mode}, got {other:?}"),
        }
    }
}

#[test]
fn test_estimate_without_breakdown_walks_filesystem() {
    let tmp = tempfile::tempdir().unwrap();
    let mut project = create_mock_project(tmp.path());
    project.breakdown = None;

    // Without a cached breakdown the estimate must equal live file bytes.
    assert_eq!(
        estimate_freed(&project, &CleanMode::Full).unwrap(),
        4096 + 2048 + 1024 + 512
    );
    assert_eq!(
        estimate_freed(&project, &CleanMode::IncrementalOnly).unwrap(),
        2048
    );
    assert_eq!(
        estimate_freed(&project, &CleanMode::DepsOnly).unwrap(),
        4096 + 1024
    );
}

#[test]
fn test_estimate_missing_target_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    fs::remove_dir_all(&project.artifact_dir).unwrap();

    assert!(estimate_freed(&project, &CleanMode::Full).is_err());
    assert!(estimate_freed(&project, &CleanMode::DepsOnly).is_err());
}

#[test]
fn test_estimate_empty_target_is_zero() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"empty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("target")).unwrap();
    let project = DiscoveredProject {
        name: "empty".to_string(),
        path: root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: root.path().join("target"),
        artifact_size: 0,
        last_modified: None,
        breakdown: None,
    };

    assert_eq!(estimate_freed(&project, &CleanMode::Full).unwrap(), 0);
}

#[cfg(unix)]
#[test]
fn test_selective_modes_reject_symlink_inside_target() {
    let project_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    let outside_file = outside_root.path().join("sentinel");
    fs::write(&outside_file, b"must survive").unwrap();

    fs::write(
        project_root.path().join("Cargo.toml"),
        "[package]\nname = \"deps-symlink\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let deps = project_root.path().join("target/debug/deps");
    fs::create_dir_all(&deps).unwrap();
    fs::write(deps.join("lib.rlib"), b"artifact").unwrap();
    symlink(&outside_file, deps.join("linked-file")).unwrap();
    let project = DiscoveredProject {
        name: "deps-symlink".to_string(),
        path: project_root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: project_root.path().join("target"),
        artifact_size: 8,
        last_modified: None,
        breakdown: None,
    };

    let result = clean_project(
        &project,
        &CleanMode::DepsOnly,
        &CleanBehavior::Delete,
        false,
    );
    assert!(
        matches!(result, deoxidizer_lib::cleaner::CleanResult::Error { .. }),
        "got {result:?}"
    );
    assert!(outside_file.exists());
    assert!(deps.join("lib.rlib").exists());
}

#[test]
fn test_full_mode_on_empty_target() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"empty-target\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("target")).unwrap();
    let project = DiscoveredProject {
        name: "empty-target".to_string(),
        path: root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: root.path().join("target"),
        artifact_size: 0,
        last_modified: None,
        breakdown: None,
    };

    // Locked-in behavior: an existing but empty target/ still resolves to a
    // Cleaned result with zero bytes (not Skipped, not Error).
    match clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false) {
        deoxidizer_lib::cleaner::CleanResult::Cleaned { bytes_freed } => {
            assert_eq!(bytes_freed, 0);
        }
        other => panic!("expected Cleaned with zero bytes, got {other:?}"),
    }
    assert!(!project.artifact_dir.exists());
}

#[test]
fn test_clean_missing_target_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    fs::remove_dir_all(&project.artifact_dir).unwrap();

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    assert!(
        matches!(result, deoxidizer_lib::cleaner::CleanResult::Error { .. }),
        "got {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn test_clean_partial_on_unreadable_subdir() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let project = create_mock_project(tmp.path());
    let locked = project.artifact_dir.join("debug");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Permissions are not enforced (e.g. running as root): premise void.
        eprintln!("premise void: running as root, perms unenforced; skipping permission test");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
    assert!(
        matches!(
            result,
            deoxidizer_lib::cleaner::CleanResult::Partial { .. }
                | deoxidizer_lib::cleaner::CleanResult::Error { .. }
        ),
        "got {result:?}"
    );
}

#[cfg(windows)]
#[test]
fn test_windows_symlink_target_rejected() {
    let project_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    let outside_target = outside_root.path().join("target");
    fs::create_dir_all(&outside_target).unwrap();
    let sentinel = outside_target.join("sentinel");
    fs::write(&sentinel, b"must survive").unwrap();

    fs::write(
        project_root.path().join("Cargo.toml"),
        "[package]\nname = \"win-symlinked\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let target = project_root.path().join("target");
    if std::os::windows::fs::symlink_dir(&outside_target, &target).is_err() {
        // No symlink privilege: fall back to a junction (needs none).
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &target.to_string_lossy(),
                &outside_target.to_string_lossy(),
            ])
            .status();
        if status.map(|status| !status.success()).unwrap_or(true) {
            return; // Environment cannot create links; nothing to verify.
        }
    }
    if fs::symlink_metadata(&target).is_err() {
        return; // No link was created; nothing to verify.
    }
    let project = DiscoveredProject {
        name: "win-symlinked".to_string(),
        path: project_root.path().to_path_buf(),
        kind: ProjectKind::RustProject,
        artifact_dir: target,
        artifact_size: 1,
        last_modified: None,
        breakdown: None,
    };

    let result = clean_project(&project, &CleanMode::Full, &CleanBehavior::Delete, false);
    assert!(
        matches!(result, deoxidizer_lib::cleaner::CleanResult::Error { .. }),
        "got {result:?}"
    );
    assert!(sentinel.exists());
}
