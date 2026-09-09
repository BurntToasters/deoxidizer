use crate::config::CleanBehavior;
use crate::project::DiscoveredProject;
use crate::scanner::validate_project_root;
use std::fs;
use std::path::{Path, PathBuf};

/// The clean mode determines what gets removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanMode {
    /// Delete entire target/ directory.
    Full,
    /// Delete debug/ but keep release/.
    DebugOnly,
    /// Delete incremental/ caches only.
    IncrementalOnly,
    /// Delete deps/ caches only.
    DepsOnly,
}

impl CleanMode {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "full" => Some(CleanMode::Full),
            "debug-only" | "debug" => Some(CleanMode::DebugOnly),
            "incremental-only" | "incremental" => Some(CleanMode::IncrementalOnly),
            "deps-only" | "deps" => Some(CleanMode::DepsOnly),
            _ => None,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            CleanMode::Full => "full (entire target/)",
            CleanMode::DebugOnly => "debug-only (target/debug/)",
            CleanMode::IncrementalOnly => "incremental-only (*/incremental/)",
            CleanMode::DepsOnly => "deps-only (*/deps/)",
        }
    }
}

impl std::fmt::Display for CleanMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CleanMode::Full => write!(f, "full"),
            CleanMode::DebugOnly => write!(f, "debug-only"),
            CleanMode::IncrementalOnly => write!(f, "incremental-only"),
            CleanMode::DepsOnly => write!(f, "deps-only"),
        }
    }
}

impl From<crate::config::DefaultMode> for CleanMode {
    fn from(mode: crate::config::DefaultMode) -> Self {
        match mode {
            crate::config::DefaultMode::Full => CleanMode::Full,
            crate::config::DefaultMode::DebugOnly => CleanMode::DebugOnly,
            crate::config::DefaultMode::IncrementalOnly => CleanMode::IncrementalOnly,
            crate::config::DefaultMode::DepsOnly => CleanMode::DepsOnly,
        }
    }
}

/// Result of a clean operation on a single project.
#[derive(Debug)]
pub enum CleanResult {
    Cleaned { bytes_freed: u64 },
    Partial { bytes_freed: u64, message: String },
    Skipped { reason: String },
    Error { message: String },
}

/// Resolve paths that should be deleted for a given project + mode.
fn paths_to_clean(project: &DiscoveredProject, mode: &CleanMode) -> Result<Vec<PathBuf>, String> {
    let target = validate_target_path(project)?;
    match mode {
        CleanMode::Full => Ok(vec![target]),
        CleanMode::DebugOnly => {
            let mut dirs = Vec::new();
            let top_debug = target.join("debug");
            if is_real_directory(&top_debug) {
                dirs.push(top_debug);
            }
            // Include target triple debug dirs (e.g. target/x86_64-pc-windows-msvc/debug)
            let entries = fs::read_dir(&target).map_err(|error| {
                format!(
                    "cannot inspect target directory {}: {error}",
                    target.display()
                )
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "cannot inspect target entry in {}: {error}",
                        target.display()
                    )
                })?;
                let path = entry.path();
                let entry_type = entry
                    .file_type()
                    .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
                if entry_type.is_dir() {
                    let name = entry.file_name();
                    if name != "debug" && name != "release" {
                        let sub_debug = path.join("debug");
                        if is_real_directory(&sub_debug) {
                            dirs.push(sub_debug);
                        }
                    }
                }
            }
            Ok(dirs)
        }
        CleanMode::IncrementalOnly => {
            let mut dirs = Vec::new();
            collect_subdirs_named(&target, "incremental", &mut dirs)?;
            Ok(dirs)
        }
        CleanMode::DepsOnly => {
            let mut dirs = Vec::new();
            collect_subdirs_named(&target, "deps", &mut dirs)?;
            Ok(dirs)
        }
    }
}

fn validate_target_path(project: &DiscoveredProject) -> Result<PathBuf, String> {
    let project_metadata = fs::symlink_metadata(&project.path)
        .map_err(|error| format!("cannot inspect project root: {error}"))?;
    if project_metadata.file_type().is_symlink() || !project_metadata.is_dir() {
        return Err("project root is not a real directory".to_string());
    }
    let project_root = project
        .path
        .canonicalize()
        .map_err(|error| format!("cannot resolve project root: {error}"))?;
    validate_project_root(&project_root, &project.name)?;

    let target_metadata = fs::symlink_metadata(&project.artifact_dir)
        .map_err(|error| format!("cannot inspect target directory: {error}"))?;
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        return Err("target directory is missing or is a symlink".to_string());
    }
    if project.artifact_dir.file_name() != Some("target".as_ref()) {
        return Err("artifact path is not named target".to_string());
    }

    let target_parent = project
        .artifact_dir
        .parent()
        .ok_or_else(|| "target directory has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("cannot resolve target parent: {error}"))?;
    if target_parent != project_root {
        return Err("target directory is not directly under project root".to_string());
    }

    let target = project
        .artifact_dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve target directory: {error}"))?;
    if target.parent() != Some(project_root.as_path()) {
        return Err("resolved target directory escaped project root".to_string());
    }
    Ok(target)
}

fn validate_clean_path(project: &DiscoveredProject, path: &Path) -> Result<(), String> {
    let target = validate_target_path(project)?;
    let relative = path
        .strip_prefix(&target)
        .map_err(|_| "clean path is outside target directory".to_string())?;
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return Err("clean path contains parent traversal".to_string());
    }

    let mut current = target.clone();
    for component in relative.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("cannot inspect clean path {}: {error}", current.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing symlinked clean path {}",
                current.display()
            ));
        }
    }

    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve clean path {}: {error}", path.display()))?;
    if !canonical.starts_with(&target) {
        return Err(format!(
            "clean path {} resolved outside target directory",
            path.display()
        ));
    }

    Ok(())
}

/// Validate a clean path and measure it in one symlink-free traversal.
fn validate_and_size(project: &DiscoveredProject, path: &Path) -> Result<u64, String> {
    validate_clean_path(project, path)?;
    let mut size = 0u64;
    for result in walkdir::WalkDir::new(path).follow_links(false).into_iter() {
        let entry = result.map_err(|error| format!("cannot inspect clean path: {error}"))?;
        if entry.file_type().is_symlink() {
            return Err(format!(
                "refusing to clean symlink {}",
                entry.path().display()
            ));
        }
        if entry.file_type().is_file() {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            size = size.saturating_add(metadata.len());
        }
    }
    Ok(size)
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// Helper to collect subdirectories with a specific name up to depth 3.
fn collect_subdirs_named(
    root: &Path,
    target_name: &str,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for result in walkdir::WalkDir::new(root)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
    {
        let entry = result.map_err(|error| format!("clean traversal failed: {error}"))?;
        if entry.file_type().is_dir() && entry.file_name() == target_name {
            out.push(entry.path().to_path_buf());
        }
    }
    Ok(())
}

/// Calculate how many bytes would be freed for a project + mode (for dry run).
pub fn estimate_freed(project: &DiscoveredProject, mode: &CleanMode) -> Result<u64, String> {
    let paths = paths_to_clean(project, mode)?;
    let mut total = 0u64;
    for path in paths {
        if !is_real_directory(&path) {
            continue;
        }
        total = total.saturating_add(validate_and_size(project, &path)?);
    }
    Ok(total)
}

/// Clean a single project's build artifacts.
pub fn clean_project(
    project: &DiscoveredProject,
    mode: &CleanMode,
    behavior: &CleanBehavior,
    dry_run: bool,
) -> CleanResult {
    let targets = match paths_to_clean(project, mode) {
        Ok(targets) => targets,
        Err(message) => return CleanResult::Error { message },
    };
    let existing: Vec<&PathBuf> = targets.iter().filter(|p| is_real_directory(p)).collect();

    if existing.is_empty() {
        return CleanResult::Skipped {
            reason: "No matching directories found".to_string(),
        };
    }

    if dry_run {
        let mut total = 0u64;
        for path in existing {
            match validate_and_size(project, path) {
                Ok(size) => total = total.saturating_add(size),
                Err(message) => return CleanResult::Error { message },
            }
        }
        return CleanResult::Cleaned { bytes_freed: total };
    }

    let mut freed = 0u64;
    let mut errors = Vec::new();
    for path in existing {
        let size = match validate_and_size(project, path) {
            Ok(size) => size,
            Err(error) => {
                errors.push(format!("Failed to validate {}: {error}", path.display()));
                continue;
            }
        };
        match remove_path(path, behavior) {
            Ok(()) => freed += size,
            Err(e) => {
                errors.push(format!("Failed to remove {}: {}", path.display(), e));
            }
        }
    }

    if !errors.is_empty() {
        if freed > 0 {
            eprintln!("  ⚠ Some files could not be removed: {}", errors.join("; "));
            CleanResult::Partial {
                bytes_freed: freed,
                message: errors.join("; "),
            }
        } else {
            CleanResult::Error {
                message: errors.join("; "),
            }
        }
    } else {
        CleanResult::Cleaned { bytes_freed: freed }
    }
}

/// Remove a path using either permanent deletion or OS trash.
fn remove_path(path: &Path, behavior: &CleanBehavior) -> Result<(), String> {
    match behavior {
        CleanBehavior::Delete => {
            std::fs::remove_dir_all(path).map_err(|e| format!("delete failed: {e}"))
        }
        CleanBehavior::Trash => trash::delete(path).map_err(|e| format!("trash failed: {e}")),
    }
}
