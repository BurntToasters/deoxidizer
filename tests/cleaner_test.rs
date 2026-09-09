use deoxidizer_lib::cleaner::{clean_project, estimate_freed, CleanMode};
use deoxidizer_lib::config::CleanBehavior;
use deoxidizer_lib::project::{DiscoveredProject, ProjectKind, TargetBreakdown};
use std::fs;

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
