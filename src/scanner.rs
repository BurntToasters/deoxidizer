use crate::config::{Config, Scope};
use crate::project::{DiscoveredProject, ProjectKind, TargetBreakdown};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
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
        // min_size_mb is denominated in MiB (1024*1024 bytes), not decimal MB.
        let Some(min_bytes) = config.min_size_mb.checked_mul(1024 * 1024) else {
            eprintln!("Warning: min_size_mb is too large; no projects included.");
            return Ok(Vec::new());
        };
        projects.retain(|p| p.artifact_size >= min_bytes);
    }

    // Apply config-level ignored projects filter. This matches the package
    // `name` (Cargo.toml `package.name`, falling back to the directory name),
    // not the filesystem path. Comparison trims surrounding whitespace and is
    // case-insensitive via lowercase.
    if !config.ignored_projects.is_empty() {
        projects.retain(|p| {
            let normalized = p.name.trim().to_lowercase();
            !config
                .ignored_projects
                .iter()
                .any(|ign| ign.trim().to_lowercase() == normalized)
        });
    }

    Ok(projects)
}

/// Scan a specific directory for projects.
///
/// Top-walk fail-closed intent: a candidate-walk `Err` aborts the whole scan
/// with `ScanError::Traversal` rather than warn-continuing (as
/// `analyze_target` does per-project), trading availability for safety so a
/// partially-visible tree never yields a silently incomplete project list.
pub fn scan_dir(root: &Path, scope: &Scope) -> Result<Vec<DiscoveredProject>, ScanError> {
    let root = validate_scan_root(root)?;
    // Cache workspace manifest re-parses: ancestor Cargo.toml files are read
    // once per scan instead of once per candidate project.
    let mut manifest_cache: HashMap<PathBuf, Option<Value>> = HashMap::new();
    let mut seen_targets: HashSet<PathBuf> = HashSet::new();

    // Collect candidate manifests first so dedup is deterministic: sorted order
    // guarantees the lexicographically-first project claims a shared target
    // dir regardless of filesystem walk order.
    // NOTE: same_file_system(true) is deliberately not set; target dirs may
    // legitimately span mount points and must still be found.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for result in WalkDir::new(&root)
        .follow_links(false)
        // Root symlinks are already rejected by validate_scan_root, so
        // follow_root_links(false) only hardens against races/odd entries.
        .follow_root_links(false)
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
        if path.file_name() != Some(OsStr::new("Cargo.toml")) {
            continue;
        }
        candidates.push(path.to_path_buf());
    }
    candidates.sort();

    let mut projects = Vec::new();
    let mut analyze_warnings = 0u32;

    for path in &candidates {
        let project_dir = match path.parent() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };

        let Some(manifest) = cached_manifest(path, &mut manifest_cache) else {
            eprintln!(
                "Warning: ignoring unreadable or invalid manifest {}",
                path.display()
            );
            continue;
        };
        if manifest.get("package").is_none() && manifest.get("workspace").is_some() {
            continue;
        }

        let Some((artifact_root, target_dir)) =
            resolve_target_dir(&project_dir, &manifest, &root, &mut manifest_cache)
        else {
            continue;
        };

        let workspace_manifest = find_workspace_manifest(&project_dir, &root, &mut manifest_cache);
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
        // by an out-of-scope manifest encountered first. Sorted candidates
        // make the lexicographically-first claimant win deterministically.
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
        if !seen_targets.insert(canonical.clone()) {
            eprintln!(
                "Warning: duplicate target directory {} for {}; keeping lexicographically-first claimant",
                canonical.display(),
                path.display(),
            );
            continue;
        }

        let name = extract_project_name(&manifest, &project_dir);
        let (artifact_size, breakdown, last_modified) = match analyze_target(&target_dir) {
            Ok(result) => result,
            Err(error) => {
                eprintln!(
                    "Warning: cannot analyze target directory {}: {error}; skipping project {}",
                    target_dir.display(),
                    name,
                );
                analyze_warnings += 1;
                continue;
            }
        };
        // Wire TargetBreakdown::total(): exclusive profile sizes must match
        // the analyzed apparent size.
        debug_assert_eq!(
            breakdown.total(),
            artifact_size,
            "breakdown total must match artifact size for {}",
            target_dir.display()
        );

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

    if analyze_warnings > 0 {
        eprintln!("Warning: skipped {analyze_warnings} project(s) due to target analysis errors.");
    }

    // Sort by size descending, breaking ties by name for determinism.
    projects.sort_by(|a, b| {
        b.artifact_size
            .cmp(&a.artifact_size)
            .then(a.name.cmp(&b.name))
    });
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
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("Warning: cannot inspect {}: {}", path.display(), error);
            return None;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    Some(path.to_path_buf())
}

/// Cached manifest read: ancestor Cargo.toml files are shared across
/// candidates, so memoize parsed (or missing/invalid) results per scan.
fn cached_manifest(path: &Path, cache: &mut HashMap<PathBuf, Option<Value>>) -> Option<Value> {
    if let Some(cached) = cache.get(path) {
        return cached.clone();
    }
    let manifest = read_manifest(path);
    cache.insert(path.to_path_buf(), manifest.clone());
    manifest
}

/// Resolve the `target/` directory for a project.
///
/// Limitation (by design, not implemented): `CARGO_TARGET_DIR` and
/// `.cargo/config.toml` `build.target-dir` overrides are not honored.
/// Only a `target/` directory directly under the project dir or under an
/// ancestor workspace root (bounded by the scan root) is recognized.
fn resolve_target_dir(
    project_dir: &Path,
    manifest: &Value,
    scan_root: &Path,
    cache: &mut HashMap<PathBuf, Option<Value>>,
) -> Option<(PathBuf, PathBuf)> {
    let local_target = project_dir.join("target");
    if let Some(target) = real_directory(&local_target) {
        return Some((project_dir.to_path_buf(), target));
    }

    // Cargo workspaces commonly share a target/ directory at workspace root.
    // Bound the ancestor walk to the canonical scan root so scanning never
    // escapes the configured tree (ancestors() would otherwise reach /).
    for ancestor in project_dir
        .ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.starts_with(scan_root))
    {
        let ancestor_manifest_path = ancestor.join("Cargo.toml");
        let Some(ancestor_manifest) = cached_manifest(&ancestor_manifest_path, cache) else {
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
fn find_workspace_manifest(
    project_dir: &Path,
    scan_root: &Path,
    cache: &mut HashMap<PathBuf, Option<Value>>,
) -> Option<Value> {
    // Bound the walk to the scan root so lookup never escapes above it.
    for ancestor in project_dir
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(scan_root))
    {
        let path = ancestor.join("Cargo.toml");
        let Some(manifest) = cached_manifest(&path, cache) else {
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
        // NOTE: same_file_system(true) is deliberately not set; workspace
        // members may span mount points.
        for result in WalkDir::new(path)
            .follow_links(false)
            // Root symlinks already rejected above; harden explicitly.
            .follow_root_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || (!is_hidden(entry) && !is_artifact_dir(entry))
            })
        {
            let entry = result.map_err(|error| format!("workspace validation failed: {error}"))?;
            if !entry.file_type().is_file() || entry.file_name() != OsStr::new("Cargo.toml") {
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
    // Note: `[dev-dependencies]` are excluded by design. A Tauri crate used
    // only for tests/examples does not make the project a Tauri app for
    // scan/clean purposes; only runtime/build dependency tables count.
    // Note: `optional = true` still counts. An optional Tauri dependency is
    // still a Tauri app (feature-gated), so no optional filtering applies.
    // Note: `tauri-plugin-*` crates alone do NOT count. Only the exact
    // `tauri` / `tauri-build` crates (by key or `package`) mark a Tauri app.
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
    // Match `package = "tauri"` on the workspace side so an inherited alias
    // (e.g. `framework.workspace = true` with `framework.package = "tauri"`)
    // is still detected, alongside direct `tauri.workspace = true`.
    // Checked across [dependencies], [build-dependencies], and all
    // [target.*.dependencies] / [target.*.build-dependencies] tables, since
    // any of them may carry `workspace = true`.
    if let Some(workspace_manifest) = workspace_manifest {
        let workspace_dependencies = workspace_manifest
            .get("workspace")
            .and_then(|value| value.get("dependencies"))
            .and_then(Value::as_table);
        if let Some(workspace_dependencies) = workspace_dependencies {
            let mut member_tables = Vec::new();
            if let Some(table) = manifest.get("dependencies").and_then(Value::as_table) {
                member_tables.push(table);
            }
            if let Some(table) = manifest.get("build-dependencies").and_then(Value::as_table) {
                member_tables.push(table);
            }
            if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
                for target in targets.values() {
                    if let Some(table) = target.get("dependencies").and_then(Value::as_table) {
                        member_tables.push(table);
                    }
                    if let Some(table) = target.get("build-dependencies").and_then(Value::as_table)
                    {
                        member_tables.push(table);
                    }
                }
            }
            let inherited = member_tables
                .into_iter()
                .flat_map(|table| table.iter())
                .any(|(name, value)| {
                    if !value
                        .get("workspace")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        return false;
                    }
                    // Renamed on the member side (`alias = { workspace = true,
                    // package = "tauri" }`) or direct `tauri.workspace = true`.
                    if is_tauri_dep_entry(name, value) {
                        return true;
                    }
                    // Inherited alias: look up the same key workspace-side and
                    // accept either the plain `tauri` key or `package = "tauri"`.
                    workspace_dependencies
                        .get(name)
                        .is_some_and(|ws_value| is_tauri_dep_entry(name, ws_value))
                });
            if inherited {
                return ProjectKind::TauriApp;
            }
        }
    }

    if dependency_tables.into_iter().any(|table| {
        table
            .iter()
            .any(|(name, value)| is_tauri_dep_entry(name, value))
    }) {
        ProjectKind::TauriApp
    } else {
        ProjectKind::RustProject
    }
}

/// Whether a `(name, dependency-value)` entry refers to `tauri`/`tauri-build`.
///
/// If `package` is present it governs: a key named `tauri` with
/// `package = "something-else"` is NOT a Tauri dependency.
fn is_tauri_dep_entry(name: &str, value: &Value) -> bool {
    if let Some(package) = value.get("package").and_then(Value::as_str) {
        return package == "tauri" || package == "tauri-build";
    }
    name == "tauri" || name == "tauri-build"
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
///
/// Size accounting notes (no behavior change): sizes are apparent sizes
/// (`metadata.len()`, i.e. `st_size`), not disk usage. Hardlinked files are
/// double-counted once per link, and sparse files / APFS clones / reflinks
/// may misreport relative to actual reclaimed disk space.
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

    // NOTE: same_file_system(true) is deliberately not set; target trees may
    // span mount points.
    for result in WalkDir::new(target_dir)
        .follow_links(false)
        // Never follow a symlinked root; validate paths reject those upfront.
        .follow_root_links(false)
        .into_iter()
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
        total_size = total_size.saturating_add(len);

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

            // Classify by parent directory components only: a file literally
            // named `debug`/`release`/`deps`/`incremental` must not affect
            // its profile bucket.
            if let Some(parent) = rel.parent() {
                for comp in parent.components() {
                    let name = comp.as_os_str();
                    if name == OsStr::new("debug") {
                        is_debug = true;
                    } else if name == OsStr::new("release") {
                        is_release = true;
                    } else if name == OsStr::new("incremental") {
                        is_incremental = true;
                    } else if name == OsStr::new("deps") {
                        is_deps = true;
                    }
                }
            }

            if is_debug {
                debug_size = debug_size.saturating_add(len);
            } else if is_release {
                release_size = release_size.saturating_add(len);
            } else {
                other_size = other_size.saturating_add(len);
            }

            if is_incremental {
                incremental_size = incremental_size.saturating_add(len);
            }
            if is_deps {
                deps_size = deps_size.saturating_add(len);
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

/// Check if a walkdir entry is a hidden directory/file.
fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    // Byte-based so non-UTF8 names are still detected as hidden.
    let bytes = entry.file_name().as_encoded_bytes();
    bytes.starts_with(b".") && bytes != b"."
}

/// Check if a walkdir entry is an artifact directory we should not recurse into.
fn is_artifact_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name();
    name == OsStr::new("target") || name == OsStr::new("node_modules") || name == OsStr::new(".git")
}
