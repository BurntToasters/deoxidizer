use crate::config::CleanBehavior;
use crate::project::DiscoveredProject;
use crate::scanner::validate_project_root;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Cargo's per-profile build lock file name.
const CARGO_LOCK_FILE: &str = ".cargo-lock";

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

/// Options that refine a clean beyond its mode.
#[derive(Debug, Clone, Copy, Default)]
pub struct CleanOptions {
    /// Measure only; never touch the filesystem.
    pub dry_run: bool,
    /// In `full` mode, keep Tauri installer bundles (`*/release/bundle`).
    pub keep_bundles: bool,
}

/// Result of a clean operation on a single project.
#[derive(Debug)]
pub enum CleanResult {
    Cleaned { bytes_freed: u64 },
    Partial { bytes_freed: u64, message: String },
    Skipped { reason: String },
    Error { message: String },
}

/// Validated set of paths a clean would remove, all inside `target`.
#[derive(Debug)]
pub struct CleanPlan {
    pub target: PathBuf,
    pub paths: Vec<PathBuf>,
}

/// Resolve and validate the paths a project + mode would remove.
///
/// The project root and `target/` are validated once; every returned path
/// is inside the canonical target, deduplicated so no path is nested inside
/// another (so removing one never invalidates a later one).
pub fn plan(
    project: &DiscoveredProject,
    mode: &CleanMode,
    options: &CleanOptions,
) -> Result<CleanPlan, String> {
    let target = validate_target_path(project)?;
    let mut paths = match mode {
        CleanMode::Full if options.keep_bundles => {
            let preserved = bundle_dirs(&target);
            let mut paths = Vec::new();
            collect_except(&target, &preserved, &mut paths)?;
            paths
        }
        CleanMode::Full => vec![target.clone()],
        CleanMode::DebugOnly => {
            let mut dirs = Vec::new();
            for profile_parent in profile_parents(&target)? {
                let debug = profile_parent.join("debug");
                if is_real_directory(&debug) {
                    dirs.push(debug);
                }
            }
            dirs
        }
        CleanMode::IncrementalOnly => collect_subdirs_named(&target, "incremental")?,
        CleanMode::DepsOnly => collect_subdirs_named(&target, "deps")?,
    };
    paths.sort();
    let mut deduped: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        if !deduped.iter().any(|kept| path.starts_with(kept)) {
            deduped.push(path);
        }
    }
    for path in &deduped {
        validate_clean_path(&target, path)
            .map_err(|error| format!("{error} (project {})", project.path.display()))?;
    }
    Ok(CleanPlan {
        target,
        paths: deduped,
    })
}

/// `target/` plus every non-`debug`/non-`release` child directory of it
/// (cross-compilation triple candidates; deliberately not narrowed to a
/// known triple list).
fn profile_parents(target: &Path) -> Result<Vec<PathBuf>, String> {
    let mut parents = vec![target.to_path_buf()];
    let entries = fs::read_dir(target).map_err(|error| {
        format!(
            "cannot inspect target directory {}: {error}",
            target.display()
        )
    })?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot inspect entry in {}: {error}", target.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        let name = entry.file_name();
        if file_type.is_dir() && name != "debug" && name != "release" {
            parents.push(entry.path());
        }
    }
    parents.sort();
    Ok(parents)
}

/// Tauri installer bundle directories: `target/release/bundle` and
/// `target/<triple>/release/bundle`.
pub fn bundle_dirs(target: &Path) -> Vec<PathBuf> {
    profile_parents(target)
        .unwrap_or_default()
        .into_iter()
        .map(|parent| parent.join("release").join("bundle"))
        .filter(|path| is_real_directory(path))
        .collect()
}

/// Collect every entry under `dir` except the `preserved` directories and
/// the directories leading to them.
fn collect_except(dir: &Path, preserved: &[PathBuf], out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("cannot inspect {}: {error}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot inspect entry in {}: {error}", dir.display()))?;
        let path = entry.path();
        if preserved.contains(&path) {
            continue;
        }
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if is_dir && preserved.iter().any(|keep| keep.starts_with(&path)) {
            collect_except(&path, preserved, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
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
    let project_root = dunce::canonicalize(&project.path).map_err(|error| {
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

    let target = dunce::canonicalize(&project.artifact_dir).map_err(|error| {
        format!(
            "cannot resolve target directory {}: {error}",
            project.artifact_dir.display()
        )
    })?;
    if target.parent() != Some(project_root.as_path()) {
        return Err(format!(
            "target directory {} is not directly under project root {}",
            project.artifact_dir.display(),
            project.path.display()
        ));
    }
    Ok(target)
}

/// Check a clean path stays inside `target`: no parent traversal, and no
/// symlink anywhere between `target` and the path's parent. The path itself
/// may be a symlink (only possible for `--keep-bundles` entries); removal
/// deletes the link, never its destination.
fn validate_clean_path(target: &Path, path: &Path) -> Result<(), String> {
    if path == target {
        return Ok(());
    }
    let relative = path.strip_prefix(target).map_err(|_| {
        format!(
            "clean path {} is outside target directory {}",
            path.display(),
            target.display()
        )
    })?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "clean path {} contains non-normal components",
            path.display()
        ));
    }
    let mut current = target.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("cannot inspect clean path {}: {error}", current.display()))?;
        let is_last = index + 1 == components.len();
        if metadata.file_type().is_symlink() && !is_last {
            return Err(format!(
                "refusing clean path through symlink {}",
                current.display()
            ));
        }
    }
    let parent = path.parent().unwrap_or(target);
    let canonical_parent = dunce::canonicalize(parent)
        .map_err(|error| format!("cannot resolve {}: {error}", parent.display()))?;
    if !canonical_parent.starts_with(target) {
        return Err(format!(
            "clean path {} resolved outside target directory {}",
            path.display(),
            target.display()
        ));
    }
    Ok(())
}

/// Apparent size of a path without following symlinks. Symlinks count as
/// zero bytes (removing a link frees no file data).
fn size_of(path: &Path) -> Result<u64, String> {
    let mut size = 0u64;
    for result in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry =
            result.map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if entry.file_type().is_file() {
            let metadata = entry
                .metadata()
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

/// Collect real directories named `target_name` at depth 1–3 inside `root`.
///
/// Depth 3 covers the deepest Cargo layout
/// `target/<triple>/<profile>/{incremental,deps}`; deeper matches are
/// intentionally left alone.
fn collect_subdirs_named(root: &Path, target_name: &str) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for result in walkdir::WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
        .min_depth(1)
        .max_depth(3)
    {
        let entry = result
            .map_err(|error| format!("clean traversal failed in {}: {error}", root.display()))?;
        if entry.file_type().is_dir() && entry.file_name() == target_name {
            out.push(entry.path().to_path_buf());
        }
    }
    Ok(out)
}

/// Report whether any Cargo build currently holds a lock inside `target`.
///
/// Cargo holds an exclusive lock on `<profile dir>/.cargo-lock` while it
/// builds. Probing takes and immediately releases a non-blocking lock on a
/// read-only handle; it never writes.
pub fn build_in_progress(target: &Path) -> bool {
    walkdir::WalkDir::new(target)
        .follow_links(false)
        .follow_root_links(false)
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == CARGO_LOCK_FILE)
        .any(|entry| match fs::File::open(entry.path()) {
            Ok(file) => matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)),
            Err(_) => false,
        })
}

/// Measure exactly what a clean would free, without touching the filesystem.
pub fn estimate_freed(project: &DiscoveredProject, mode: &CleanMode) -> Result<u64, String> {
    estimate_with(project, mode, &CleanOptions::default())
}

/// Live measurement of a clean plan (the canonical dry-run byte count).
pub fn estimate_with(
    project: &DiscoveredProject,
    mode: &CleanMode,
    options: &CleanOptions,
) -> Result<u64, String> {
    let plan = plan(project, mode, options)?;
    plan.paths.iter().try_fold(0u64, |total, path| {
        size_of(path).map(|size| total.saturating_add(size))
    })
}

/// Clean a single project's build artifacts.
pub fn clean_project(
    project: &DiscoveredProject,
    mode: &CleanMode,
    behavior: &CleanBehavior,
    dry_run: bool,
) -> CleanResult {
    clean_project_with(
        project,
        mode,
        behavior,
        &CleanOptions {
            dry_run,
            ..CleanOptions::default()
        },
    )
}

/// Clean a single project's build artifacts with explicit options.
pub fn clean_project_with(
    project: &DiscoveredProject,
    mode: &CleanMode,
    behavior: &CleanBehavior,
    options: &CleanOptions,
) -> CleanResult {
    let plan = match plan(project, mode, options) {
        Ok(plan) => plan,
        Err(message) => return CleanResult::Error { message },
    };
    if plan.paths.is_empty() {
        return CleanResult::Skipped {
            reason: "No matching directories found".to_string(),
        };
    }
    if build_in_progress(&plan.target) {
        return CleanResult::Skipped {
            reason: "build in progress (Cargo build lock is held)".to_string(),
        };
    }

    let mut freed = 0u64;
    let mut errors = Vec::new();
    for path in &plan.paths {
        let size = match size_of(path) {
            Ok(size) => size,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if options.dry_run {
            freed = freed.saturating_add(size);
            continue;
        }
        match remove_path(path, behavior) {
            Ok(()) => freed = freed.saturating_add(size),
            Err(error) => {
                // Credit whatever a failed recursive delete did remove.
                let remaining = if fs::symlink_metadata(path).is_ok() {
                    size_of(path).unwrap_or(size)
                } else {
                    0
                };
                freed = freed.saturating_add(size.saturating_sub(remaining));
                errors.push(format!("failed to remove {}: {error}", path.display()));
            }
        }
    }

    if errors.is_empty() {
        return CleanResult::Cleaned { bytes_freed: freed };
    }
    let message = errors.join("; ");
    if freed > 0 {
        CleanResult::Partial {
            bytes_freed: freed,
            message,
        }
    } else {
        CleanResult::Error { message }
    }
}

/// Remove a path using either permanent deletion or OS trash.
///
/// `remove_dir_all` and `remove_file` never follow symlinks: a link inside
/// `target/` is removed as a link and its destination is untouched.
fn remove_path(path: &Path, behavior: &CleanBehavior) -> Result<(), String> {
    match behavior {
        CleanBehavior::Delete => {
            let metadata =
                fs::symlink_metadata(path).map_err(|error| format!("delete failed: {error}"))?;
            if metadata.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            }
            .map_err(|error| format!("delete failed: {error}"))
        }
        CleanBehavior::Trash => trash_context()
            .delete(path)
            .map_err(|error| format!("trash failed: {error}")),
    }
}

/// Trash context; on macOS uses `NSFileManager` instead of scripting
/// Finder, which avoids the Automation permission prompt and works headless.
fn trash_context() -> trash::TrashContext {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context
}
