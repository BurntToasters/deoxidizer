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
                    "cannot inspect target directory {} of project {}: {error}",
                    target.display(),
                    project.path.display()
                )
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "cannot inspect target entry in {} of project {}: {error}",
                        target.display(),
                        project.path.display()
                    )
                })?;
                let path = entry.path();
                let entry_type = entry.file_type().map_err(|error| {
                    format!(
                        "cannot inspect {} of project {}: {error}",
                        path.display(),
                        project.path.display()
                    )
                })?;
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
    let project_metadata = fs::symlink_metadata(&project.path).map_err(|error| {
        format!(
            "cannot inspect project root {}: {error}",
            project.path.display()
        )
    })?;
    if project_metadata.file_type().is_symlink() || !project_metadata.is_dir() {
        return Err(format!(
            "project root {} is not a real directory",
            project.path.display()
        ));
    }
    let project_root = project.path.canonicalize().map_err(|error| {
        format!(
            "cannot resolve project root {}: {error}",
            project.path.display()
        )
    })?;
    validate_project_root(&project_root, &project.name).map_err(|error| {
        format!(
            "project {} failed validation: {error}",
            project.path.display()
        )
    })?;

    let target_metadata = fs::symlink_metadata(&project.artifact_dir).map_err(|error| {
        format!(
            "cannot inspect target directory {}: {error}",
            project.artifact_dir.display()
        )
    })?;
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        return Err(format!(
            "target directory {} is missing or is a symlink",
            project.artifact_dir.display()
        ));
    }
    if project.artifact_dir.file_name() != Some("target".as_ref()) {
        return Err(format!(
            "artifact path {} is not named target",
            project.artifact_dir.display()
        ));
    }

    let target_parent = project
        .artifact_dir
        .parent()
        .ok_or_else(|| {
            format!(
                "target directory {} has no parent",
                project.artifact_dir.display()
            )
        })?
        .canonicalize()
        .map_err(|error| {
            format!(
                "cannot resolve target parent of {}: {error}",
                project.artifact_dir.display()
            )
        })?;
    if target_parent != project_root {
        return Err(format!(
            "target directory {} is not directly under project root {}",
            project.artifact_dir.display(),
            project.path.display()
        ));
    }

    let target = project.artifact_dir.canonicalize().map_err(|error| {
        format!(
            "cannot resolve target directory {}: {error}",
            project.artifact_dir.display()
        )
    })?;
    if target.parent() != Some(project_root.as_path()) {
        return Err(format!(
            "resolved target directory {} escaped project root {}",
            target.display(),
            project.path.display()
        ));
    }
    Ok(target)
}

fn validate_clean_path(project: &DiscoveredProject, path: &Path) -> Result<(), String> {
    let target = validate_target_path(project)?;
    let relative = path.strip_prefix(&target).map_err(|_| {
        format!(
            "clean path {} is outside target directory {} of project {}",
            path.display(),
            target.display(),
            project.path.display()
        )
    })?;
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(format!(
            "clean path {} contains parent traversal (project {})",
            path.display(),
            project.path.display()
        ));
    }

    let mut current = target.clone();
    for component in relative.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "cannot inspect clean path {} of project {}: {error}",
                current.display(),
                project.path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing symlinked clean path {} of project {}",
                current.display(),
                project.path.display()
            ));
        }
    }

    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "cannot resolve clean path {} of project {}: {error}",
            path.display(),
            project.path.display()
        )
    })?;
    if !canonical.starts_with(&target) {
        return Err(format!(
            "clean path {} of project {} resolved outside target directory {}",
            path.display(),
            project.path.display(),
            target.display()
        ));
    }

    Ok(())
}

/// Validate a clean path and measure it in one symlink-free traversal.
fn validate_and_size(project: &DiscoveredProject, path: &Path) -> Result<u64, String> {
    validate_clean_path(project, path)?;
    let mut size = 0u64;
    // NOTE: same_file_system(true) is deliberately not set; target trees may
    // span mount points.
    for result in walkdir::WalkDir::new(path)
        .follow_links(false)
        // Root symlinks already rejected by validation; harden explicitly.
        .follow_root_links(false)
        .into_iter()
    {
        let entry = result.map_err(|error| {
            format!(
                "cannot inspect clean path {} of project {}: {error}",
                path.display(),
                project.path.display()
            )
        })?;
        if entry.file_type().is_symlink() {
            return Err(format!(
                "refusing to clean symlink {} of project {}",
                entry.path().display(),
                project.path.display()
            ));
        }
        if entry.file_type().is_file() {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!(
                    "cannot inspect {} of project {}: {error}",
                    entry.path().display(),
                    project.path.display()
                )
            })?;
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

/// Helper to collect subdirectories with a specific name.
///
/// Fail-closed depth cap: `max_depth(3)` covers the deepest documented Cargo
/// triple layout `target/<triple>/<profile>/{incremental,deps}` (exactly
/// depth 3) while intentionally under-cleaning deeper layouts rather than
/// risking unbounded traversal/deletion. Name matching plus
/// `validate_clean_path` symlink/containment re-checks still apply at clean
/// time.
fn collect_subdirs_named(
    root: &Path,
    target_name: &str,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    // NOTE: same_file_system(true) is deliberately not set; target trees may
    // span mount points.
    for result in walkdir::WalkDir::new(root)
        .follow_links(false)
        // Root symlinks already rejected by validation; harden explicitly.
        .follow_root_links(false)
        .max_depth(3)
        .into_iter()
    {
        let entry = result
            .map_err(|error| format!("clean traversal failed in {}: {error}", root.display()))?;
        if entry.file_type().is_dir() && entry.file_name() == target_name {
            // Pre-filter symlinks fail-closed before push; validate_clean_path
            // performs the authoritative re-check at clean time.
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            out.push(entry.path().to_path_buf());
        }
    }
    Ok(())
}

/// Calculate how many bytes would be freed for a project + mode (for dry run).
///
/// Discovery shares [`paths_to_clean`] with the actual clean path, so dry-run
/// and clean agree on *which* directories are targeted under the same
/// depth-aware (`max_depth(3)`, fail-closed) discovery rule. When a scan-time
/// `breakdown` is present the estimate reuses it for speed; that snapshot was
/// built with the same depth rule at scan time but may be stale if the target
/// tree changed afterwards, so `clean --dry-run` (which re-measures via
/// [`validate_and_size`]) remains the canonical reclaimable-byte source. The
/// `None` fallback path (and all actual cleans) re-measures live via
/// [`validate_and_size`] using the same depth-aware helper. Empty discovery
/// returns 0 without consulting the snapshot.
pub fn estimate_freed(project: &DiscoveredProject, mode: &CleanMode) -> Result<u64, String> {
    let paths = paths_to_clean(project, mode)?;
    if paths.is_empty() {
        return Ok(0);
    }
    if let Some(breakdown) = project.breakdown.as_ref() {
        // NOTE: stale-breakdown risk documented above; fresh measurement
        // happens at clean time via validate_and_size.
        return Ok(match mode {
            CleanMode::Full => project.artifact_size,
            CleanMode::DebugOnly => breakdown.debug_size,
            CleanMode::IncrementalOnly => breakdown.incremental_size,
            CleanMode::DepsOnly => breakdown.deps_size,
        });
    }
    // Warn-and-continue per path: sum successes, fail only if all fail.
    let mut total = 0u64;
    let mut failures = 0usize;
    let mut last_error = String::new();
    for path in &paths {
        if !is_real_directory(path) {
            continue;
        }
        match validate_and_size(project, path) {
            Ok(size) => total = total.saturating_add(size),
            Err(error) => {
                eprintln!(
                    "Warning: cannot estimate {} of project {}: {error}",
                    path.display(),
                    project.path.display()
                );
                failures += 1;
                last_error = error;
            }
        }
    }
    if failures > 0 && total == 0 {
        // Distinguish "nothing measurable" from "empty": if every attempted
        // path failed, propagate an error so callers exit non-zero.
        let attempted = paths.iter().any(|p| is_real_directory(p));
        if attempted {
            return Err(last_error);
        }
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
    let existing_count = existing.len();

    if dry_run {
        // Warn-and-continue per path: sum successes, fail only if all fail.
        let mut total = 0u64;
        let mut errors = Vec::new();
        for path in &existing {
            match validate_and_size(project, path) {
                Ok(size) => total = total.saturating_add(size),
                Err(message) => {
                    eprintln!(
                        "Warning: cannot estimate {} of project {}: {message}",
                        path.display(),
                        project.path.display()
                    );
                    errors.push(message);
                }
            }
        }
        if !errors.is_empty() && total == 0 {
            return CleanResult::Error {
                message: errors.join("; "),
            };
        }
        if !errors.is_empty() {
            return CleanResult::Partial {
                bytes_freed: total,
                message: errors.join("; "),
            };
        }
        return CleanResult::Cleaned { bytes_freed: total };
    }

    let mut freed = 0u64;
    let mut errors = Vec::new();
    for path in &existing {
        let size = match validate_and_size(project, path) {
            Ok(size) => size,
            Err(error) => {
                errors.push(format!("Failed to validate {}: {error}", path.display()));
                continue;
            }
        };
        match remove_path(path, behavior) {
            Ok(()) => freed = freed.saturating_add(size),
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
        if matches!(behavior, CleanBehavior::Trash) {
            eprintln!(
                "Moved {existing_count} artifact(s) of project {} to Trash (freed ~{freed} bytes; empty Trash to reclaim disk space).",
                project.path.display(),
            );
        }
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
