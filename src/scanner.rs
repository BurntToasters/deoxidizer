use crate::config::{Config, Scope};
use crate::project::{DiscoveredProject, ProjectKind, TargetBreakdown};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use toml::Value;
use walkdir::WalkDir;

/// Errors encountered while scanning configured project roots.
#[derive(Debug)]
pub enum ScanError {
    Root { path: PathBuf, message: String },
    Traversal(String),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root { path, message } => write!(f, "cannot scan {}: {message}", path.display()),
            Self::Traversal(message) => write!(f, "scan traversal failed: {message}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Scan the configured directory for Rust/Tauri projects with build artifacts.
pub fn scan(config: &Config) -> Result<Vec<DiscoveredProject>, ScanError> {
    let root = config.projects_path();
    let mut projects = scan_dir(&root, &config.scope)?;

    // Apply config-level min size filter
    if config.min_size_mb > 0 {
        let Some(min_bytes) = config.min_size_mb.checked_mul(1024 * 1024) else {
            eprintln!("Warning: min_size_mb is too large; no projects included.");
            return Ok(Vec::new());
        };
        projects.retain(|p| p.artifact_size >= min_bytes);
    }

    // Apply config-level ignored projects filter
    if !config.ignored_projects.is_empty() {
        projects.retain(|p| {
            !config
                .ignored_projects
                .iter()
                .any(|ign| ign.eq_ignore_ascii_case(&p.name))
        });
    }

    Ok(projects)
}

/// Scan a specific directory for projects.
pub fn scan_dir(root: &Path, scope: &Scope) -> Result<Vec<DiscoveredProject>, ScanError> {
    let root = validate_scan_root(root)?;

    let mut projects = Vec::new();
    let mut seen_targets: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for result in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || (!is_hidden(e) && !is_artifact_dir(e)))
    {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                return Err(ScanError::Traversal(error.to_string()));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if path.file_name() != Some("Cargo.toml".as_ref()) {
            continue;
        }

        let project_dir = match path.parent() {
            Some(p) => p,
            None => continue,
        };

        let Some(manifest) = read_manifest(path) else {
            eprintln!(
                "Warning: ignoring unreadable or invalid manifest {}",
                path.display()
            );
            continue;
        };
        if manifest.get("package").is_none() && manifest.get("workspace").is_some() {
            continue;
        }

        let Some((artifact_root, target_dir)) = resolve_target_dir(project_dir, &manifest) else {
            continue;
        };

        let workspace_manifest = find_workspace_manifest(project_dir);
        let kind = detect_project_kind(&manifest, workspace_manifest.as_ref());

        // Apply scope filter
        match scope {
            Scope::TauriOnly => {
                if kind != ProjectKind::TauriApp {
                    continue;
                }
            }
            Scope::RustOnly => {
                if kind != ProjectKind::RustProject {
                    continue;
                }
            }
            Scope::TauriAndRust => {
                // Include both Tauri and plain Rust
            }
        }

        // Avoid duplicates (e.g. workspace members sharing a target dir) only
        // after scope filtering, so an in-scope workspace member is not hidden
        // by an out-of-scope manifest encountered first.
        let canonical = match target_dir.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                eprintln!(
                    "Warning: cannot resolve target directory {}: {}",
                    target_dir.display(),
                    error
                );
                continue;
            }
        };
        if !seen_targets.insert(canonical) {
            continue;
        }

        let name = extract_project_name(&manifest, project_dir);
        let (artifact_size, breakdown, last_modified) = analyze_target(&target_dir)?;

        projects.push(DiscoveredProject {
            name,
            path: artifact_root,
            kind,
            artifact_dir: target_dir,
            artifact_size,
            last_modified,
            breakdown: Some(breakdown),
        });
    }

    // Sort by size descending
    projects.sort_by_key(|a| std::cmp::Reverse(a.artifact_size));
    Ok(projects)
}

/// Reject symlinked configured roots before canonicalizing them.
fn validate_scan_root(root: &Path) -> Result<PathBuf, ScanError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| ScanError::Root {
        path: root.to_path_buf(),
        message: format!("metadata failed: {error}"),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ScanError::Root {
            path: root.to_path_buf(),
            message: "configured scan root is a symlink".to_string(),
        });
    }
    if !metadata.is_dir() {
        return Err(ScanError::Root {
            path: root.to_path_buf(),
            message: "configured scan root is not a directory".to_string(),
        });
    }
    root.canonicalize().map_err(|error| ScanError::Root {
        path: root.to_path_buf(),
        message: format!("path resolution failed: {error}"),
    })
}

fn read_manifest(path: &Path) -> Option<Value> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "Warning: cannot read manifest {}: {}",
                path.display(),
                error
            );
            return None;
        }
    };
    match content.parse::<Value>() {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            eprintln!(
                "Warning: cannot parse manifest {}: {}",
                path.display(),
                error
            );
            None
        }
    }
}

fn real_directory(path: &Path) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    Some(path.to_path_buf())
}

fn resolve_target_dir(project_dir: &Path, manifest: &Value) -> Option<(PathBuf, PathBuf)> {
    let local_target = project_dir.join("target");
    if fs::symlink_metadata(&local_target).is_ok() {
        return real_directory(&local_target).map(|target| (project_dir.to_path_buf(), target));
    }

    // Cargo workspaces commonly share a target/ directory at workspace root.
    for ancestor in project_dir.ancestors().skip(1) {
        let ancestor_manifest_path = ancestor.join("Cargo.toml");
        let Some(ancestor_manifest) = read_manifest(&ancestor_manifest_path) else {
            continue;
        };
        if ancestor_manifest.get("workspace").is_none() {
            continue;
        }
        let target = ancestor.join("target");
        if let Some(target) = real_directory(&target) {
            return Some((ancestor.to_path_buf(), target));
        }
    }

    // A workspace declaration in this manifest can still use its own target.
    if manifest.get("workspace").is_some() {
        return real_directory(&local_target).map(|target| (project_dir.to_path_buf(), target));
    }
    None
}

/// Detect whether a parsed Cargo manifest indicates a Tauri project.
fn find_workspace_manifest(project_dir: &Path) -> Option<Value> {
    for ancestor in project_dir.ancestors() {
        let path = ancestor.join("Cargo.toml");
        let Some(manifest) = read_manifest(&path) else {
            continue;
        };
        if manifest.get("workspace").is_some() {
            return Some(manifest);
        }
    }
    None
}

/// Revalidate that a cleaner-supplied root is a real Cargo project/workspace.
pub fn validate_project_root(path: &Path, expected_name: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("project root is not a real directory".to_string());
    }
    let manifest_path = path.join("Cargo.toml");
    let manifest = read_manifest(&manifest_path)
        .ok_or_else(|| "project root has no valid Cargo.toml".to_string())?;
    if manifest.get("package").is_none() && manifest.get("workspace").is_none() {
        return Err("project root is not a Cargo package or workspace".to_string());
    }
    if manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|name| name == expected_name)
    {
        return Ok(());
    }
    if manifest.get("workspace").is_some() {
        for result in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || (!is_hidden(entry) && !is_artifact_dir(entry))
            })
        {
            let entry = result.map_err(|error| format!("workspace validation failed: {error}"))?;
            if !entry.file_type().is_file() || entry.file_name() != "Cargo.toml" {
                continue;
            }
            let Some(member) = read_manifest(entry.path()) else {
                continue;
            };
            if member
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|name| name == expected_name)
            {
                return Ok(());
            }
        }
    }
    Err(format!(
        "project identity does not match Cargo project {expected_name}"
    ))
}

fn detect_project_kind(manifest: &Value, workspace_manifest: Option<&Value>) -> ProjectKind {
    let mut dependency_tables = Vec::new();

    if let Some(table) = manifest.get("dependencies").and_then(Value::as_table) {
        dependency_tables.push(table);
    }
    if let Some(table) = manifest.get("build-dependencies").and_then(Value::as_table) {
        dependency_tables.push(table);
    }
    if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            if let Some(table) = target.get("dependencies").and_then(Value::as_table) {
                dependency_tables.push(table);
            }
            if let Some(table) = target.get("build-dependencies").and_then(Value::as_table) {
                dependency_tables.push(table);
            }
        }
    }
    if let Some(workspace) = manifest.get("workspace") {
        if let Some(table) = workspace.get("dependencies").and_then(Value::as_table) {
            dependency_tables.push(table);
        }
    }

    // Members may inherit renamed dependencies from [workspace.dependencies].
    if let Some(workspace_manifest) = workspace_manifest {
        let workspace_dependencies = workspace_manifest
            .get("workspace")
            .and_then(|value| value.get("dependencies"))
            .and_then(Value::as_table);
        if let (Some(member_dependencies), Some(workspace_dependencies)) = (
            manifest.get("dependencies").and_then(Value::as_table),
            workspace_dependencies,
        ) {
            if member_dependencies.iter().any(|(name, value)| {
                value
                    .get("workspace")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && workspace_dependencies
                        .get(name)
                        .is_some_and(is_tauri_dependency)
            }) {
                return ProjectKind::TauriApp;
            }
        }
    }

    if dependency_tables.into_iter().any(|table| {
        table.iter().any(|(name, value)| {
            name == "tauri" || name == "tauri-build" || is_tauri_dependency(value)
        })
    }) {
        ProjectKind::TauriApp
    } else {
        ProjectKind::RustProject
    }
}

fn is_tauri_dependency(value: &Value) -> bool {
    // Dependency aliases retain the real crate name in `package`.
    value
        .get("package")
        .and_then(Value::as_str)
        .is_some_and(|name| name == "tauri" || name == "tauri-build")
}

/// Extract the project name from a parsed Cargo manifest.
fn extract_project_name(manifest: &Value, project_dir: &Path) -> String {
    manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            project_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Analyze a target/ directory in a single traversal, computing total size,
/// breakdown (including target triples), and newest modification time.
pub fn analyze_target(
    target_dir: &Path,
) -> Result<(u64, TargetBreakdown, Option<SystemTime>), ScanError> {
    let mut total_size = 0u64;
    let mut debug_size = 0u64;
    let mut release_size = 0u64;
    let mut incremental_size = 0u64;
    let mut deps_size = 0u64;
    let mut other_size = 0u64;
    let mut last_modified: Option<SystemTime> = None;

    for result in WalkDir::new(target_dir).follow_links(false).into_iter() {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                return Err(ScanError::Traversal(error.to_string()));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ScanError::Traversal(format!(
                    "cannot inspect {}: {error}",
                    entry.path().display()
                )));
            }
        };
        let len = metadata.len();
        total_size += len;

        if let Ok(mtime) = metadata.modified() {
            last_modified = Some(match last_modified {
                Some(prev) => prev.max(mtime),
                None => mtime,
            });
        }

        if let Ok(rel) = entry.path().strip_prefix(target_dir) {
            let mut is_debug = false;
            let mut is_release = false;
            let mut is_incremental = false;
            let mut is_deps = false;

            for comp in rel.components() {
                let comp_str = comp.as_os_str().to_string_lossy();
                if comp_str == "debug" {
                    is_debug = true;
                } else if comp_str == "release" {
                    is_release = true;
                } else if comp_str == "incremental" {
                    is_incremental = true;
                } else if comp_str == "deps" {
                    is_deps = true;
                }
            }

            if is_debug {
                debug_size += len;
            } else if is_release {
                release_size += len;
            } else {
                other_size += len;
            }

            if is_incremental {
                incremental_size += len;
            }
            if is_deps {
                deps_size += len;
            }
        }
    }

    Ok((
        total_size,
        TargetBreakdown {
            debug_size,
            release_size,
            incremental_size,
            deps_size,
            other_size,
        },
        last_modified,
    ))
}

/// Calculate the total size of a directory in bytes.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return 0;
    }
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|result| match result {
            Ok(entry) => Some(entry),
            Err(error) => {
                eprintln!("Warning: size traversal error: {error}");
                None
            }
        })
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| fs::symlink_metadata(entry.path()).ok())
        .filter(|metadata| !metadata.file_type().is_symlink())
        .map(|metadata| metadata.len())
        .sum()
}

/// Check if a walkdir entry is a hidden directory/file.
fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.') && s != ".")
        .unwrap_or(false)
}

/// Check if a walkdir entry is an artifact directory we should not recurse into.
fn is_artifact_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    matches!(name.as_ref(), "target" | "node_modules" | ".git")
}
