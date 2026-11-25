use crate::config::{Config, Scope};
use crate::project::{DiscoveredProject, ProjectKind, TargetBreakdown};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;
use toml::Value;
use walkdir::WalkDir;

/// Maximum number of skipped-folder paths printed individually.
const MAX_SKIPPED_SHOWN: usize = 5;

/// Signature line Cargo writes at the start of `CACHEDIR.TAG` in every target dir.
pub(crate) const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Cargo writes `.rustc_info.json` into every target directory it builds.
const RUSTC_INFO_FILE: &str = ".rustc_info.json";

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
    scan_with_report(config).map(|report| report.projects)
}

/// Projects found by a scan plus the number of projects whose `target/`
/// could not be analyzed (reported as warnings and left out).
pub struct ScanReport {
    pub projects: Vec<DiscoveredProject>,
    pub analysis_failures: usize,
}

/// Open a regular file without following its final path component. Manifests,
/// Cargo markers, and lock files use this shared gate so a symlink can never
/// establish project or target eligibility.
pub(crate) fn open_regular_file_nofollow(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{open, Mode, OFlags};
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        File::from(fd)
    };

    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // OPEN_REPARSE_POINT opens the link itself instead of its destination.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?
    };

    #[cfg(not(any(unix, windows)))]
    let file = {
        let before = fs::symlink_metadata(path)?;
        if before.file_type().is_symlink() || !before.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a regular file",
            ));
        }
        let file = OpenOptions::new().read(true).open(path)?;
        let after = file.metadata()?;
        if !after.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "opened path is not a regular file",
            ));
        }
        file
    };

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "opened path is not a regular file",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "opened path is a reparse point",
            ));
        }
    }
    #[cfg(unix)]
    {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "opened path is a symbolic link",
            ));
        }
    }
    Ok(file)
}

/// Like [`scan`], but also reports projects that had to be skipped.
pub fn scan_with_report(config: &Config) -> Result<ScanReport, ScanError> {
    let root = config.projects_path();
    let ScanReport {
        mut projects,
        analysis_failures,
    } = scan_dir_report_for_ignores(&root, &config.scope, &config.ignored_projects)?;

    // Apply config-level min size filter
    if config.min_size_mb > 0 {
        // min_size_mb is denominated in MiB (1024*1024 bytes), not decimal MB.
        let Some(min_bytes) = config.min_size_mb.checked_mul(1024 * 1024) else {
            eprintln!("Warning: min_size_mb is too large; no projects included.");
            return Ok(ScanReport {
                projects: Vec::new(),
                analysis_failures,
            });
        };
        projects.retain(|p| p.artifact_size >= min_bytes);
    }

    // Ignored names match the project name or any package building into the
    // same target: ignoring one workspace member protects the shared target.
    // Comparison trims whitespace and is case-insensitive.
    if !config.ignored_projects.is_empty() {
        let ignored: Vec<String> = config
            .ignored_projects
            .iter()
            .map(|name| name.trim().to_lowercase())
            .collect();
        projects.retain(|p| {
            !std::iter::once(&p.name)
                .chain(p.members.iter())
                .any(|name| ignored.contains(&name.trim().to_lowercase()))
        });
    }

    Ok(ScanReport {
        projects,
        analysis_failures,
    })
}

/// Packages and owner of one `target/` directory, gathered during a scan.
struct TargetGroup {
    owner: PathBuf,
    target: PathBuf,
    members: Vec<(String, ProjectKind)>,
    ignored_workspace_member: bool,
    unresolved_workspace_member: bool,
}

/// Scan a specific directory for projects.
///
/// The configured root must be a real, readable directory (fail-closed).
/// Unreadable folders below it are skipped with a warning: the scan is
/// read-only, so a partial view can only under-report, never over-delete.
pub fn scan_dir(root: &Path, scope: &Scope) -> Result<Vec<DiscoveredProject>, ScanError> {
    scan_dir_report(root, scope).map(|report| report.projects)
}

/// Like [`scan_dir`], but also reports projects that had to be skipped.
pub fn scan_dir_report(root: &Path, scope: &Scope) -> Result<ScanReport, ScanError> {
    scan_dir_report_for_ignores(root, scope, &[])
}

fn scan_dir_report_for_ignores(
    root: &Path,
    scope: &Scope,
    ignored_projects: &[String],
) -> Result<ScanReport, ScanError> {
    let root = validate_scan_root(root)?;
    let mut manifest_cache: HashMap<PathBuf, Option<Value>> = HashMap::new();
    let mut unmarked_warned: HashSet<PathBuf> = HashSet::new();

    // Collect candidate manifests first so grouping is deterministic.
    // NOTE: same_file_system(true) is deliberately not set; target dirs may
    // legitimately span mount points and must still be found.
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for result in WalkDir::new(&root)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || (!is_hidden(e) && !is_artifact_dir(e)))
    {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) if error.depth() > 0 => {
                let path = error
                    .path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| error.to_string());
                skipped.push(path);
                continue;
            }
            Err(error) => return Err(ScanError::Traversal(error.to_string())),
        };
        if entry.file_type().is_file() && entry.file_name() == OsStr::new("Cargo.toml") {
            candidates.push(entry.path().to_path_buf());
        }
    }
    report_skipped(&skipped);
    candidates.sort();

    // Group every package by the target directory it builds into.
    let mut groups: BTreeMap<PathBuf, TargetGroup> = BTreeMap::new();
    let mut workspaces = Vec::new();
    for path in &candidates {
        let Some(project_dir) = path.parent() else {
            continue;
        };
        let Some(manifest) = cached_manifest(path, &mut manifest_cache) else {
            eprintln!(
                "Warning: ignoring unreadable or invalid manifest {}",
                crate::display::sanitize(&path.display().to_string())
            );
            continue;
        };
        let has_package = manifest.get("package").is_some();
        if !has_package && manifest.get("workspace").is_none() {
            continue;
        }
        if manifest.get("workspace").is_some() {
            workspaces.push((project_dir.to_path_buf(), manifest.clone()));
        }
        let Some((owner, target_dir)) = resolve_target_dir(
            project_dir,
            &manifest,
            &root,
            &mut manifest_cache,
            &mut unmarked_warned,
        ) else {
            continue;
        };
        let canonical = match dunce::canonicalize(&target_dir) {
            Ok(canonical) => canonical,
            Err(error) => {
                eprintln!(
                    "Warning: cannot resolve target directory {}: {}",
                    crate::display::sanitize(&target_dir.display().to_string()),
                    crate::display::sanitize(&error.to_string())
                );
                continue;
            }
        };
        let group = groups.entry(canonical).or_insert_with(|| TargetGroup {
            owner: owner.clone(),
            target: target_dir.clone(),
            members: Vec::new(),
            ignored_workspace_member: false,
            unresolved_workspace_member: false,
        });
        if has_package {
            let workspace_manifest =
                find_workspace_manifest(project_dir, &root, &mut manifest_cache);
            let kind = detect_project_kind(&manifest, workspace_manifest.as_ref());
            group
                .members
                .push((extract_project_name(&manifest, project_dir), kind));
        }
    }

    augment_workspace_members(
        &mut groups,
        &workspaces,
        &mut manifest_cache,
        &mut unmarked_warned,
        ignored_projects,
    );

    let mut projects = Vec::new();
    let mut analyze_warnings = 0usize;
    for group in groups.into_values() {
        if group.ignored_workspace_member {
            continue;
        }
        if group.unresolved_workspace_member && !ignored_projects.is_empty() {
            eprintln!(
                "Warning: skipping workspace target {} because a declared member could not be resolved while ignored-project protection is configured.",
                crate::display::sanitize(&group.target.display().to_string())
            );
            continue;
        }
        let kind = if group
            .members
            .iter()
            .any(|(_, kind)| *kind == ProjectKind::TauriApp)
        {
            ProjectKind::TauriApp
        } else {
            ProjectKind::RustProject
        };
        let in_scope = match scope {
            Scope::TauriOnly => kind == ProjectKind::TauriApp,
            Scope::RustOnly => kind == ProjectKind::RustProject,
            Scope::TauriAndRust => true,
        };
        if !in_scope {
            continue;
        }

        let owner_manifest = cached_manifest(&group.owner.join("Cargo.toml"), &mut manifest_cache);
        let name = owner_manifest
            .as_ref()
            .and_then(package_name)
            .unwrap_or_else(|| directory_name(&group.owner));
        let (artifact_size, breakdown, last_modified) = match analyze_target(&group.target) {
            Ok(result) => result,
            Err(error) => {
                eprintln!(
                    "Warning: cannot analyze target directory {}: {}; skipping project {}",
                    crate::display::sanitize(&group.target.display().to_string()),
                    crate::display::sanitize(&error.to_string()),
                    crate::display::sanitize(&name),
                );
                analyze_warnings += 1;
                continue;
            }
        };
        debug_assert_eq!(breakdown.total(), artifact_size);

        let mut members: Vec<String> = group.members.into_iter().map(|(name, _)| name).collect();
        members.sort();
        members.dedup();
        projects.push(DiscoveredProject {
            name,
            path: group.owner,
            kind,
            artifact_dir: group.target,
            artifact_size,
            last_modified,
            breakdown: Some(breakdown),
            members,
        });
    }

    if analyze_warnings > 0 {
        eprintln!("Warning: skipped {analyze_warnings} project(s) due to target analysis errors.");
    }

    // Sort by size descending, breaking ties by name then path.
    projects.sort_by(|a, b| {
        b.artifact_size
            .cmp(&a.artifact_size)
            .then(a.name.cmp(&b.name))
            .then(a.path.cmp(&b.path))
    });
    Ok(ScanReport {
        projects,
        analysis_failures: analyze_warnings,
    })
}

fn report_skipped(skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    eprintln!(
        "Warning: skipped {} unreadable folder(s) while scanning:",
        skipped.len()
    );
    for path in skipped.iter().take(MAX_SKIPPED_SHOWN) {
        eprintln!("  - {}", crate::display::sanitize(path));
    }
    if skipped.len() > MAX_SKIPPED_SHOWN {
        eprintln!("  ... and {} more", skipped.len() - MAX_SKIPPED_SHOWN);
    }
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
    fs::read_dir(root).map_err(|error| ScanError::Root {
        path: root.to_path_buf(),
        message: format!("cannot read directory: {error}"),
    })?;
    dunce::canonicalize(root).map_err(|error| ScanError::Root {
        path: root.to_path_buf(),
        message: format!("path resolution failed: {error}"),
    })
}

fn read_manifest(path: &Path) -> Option<Value> {
    let mut content = String::new();
    let mut file = match open_regular_file_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "Warning: cannot read manifest {}: {}",
                crate::display::sanitize(&path.display().to_string()),
                crate::display::sanitize(&error.to_string())
            );
            return None;
        }
    };
    if let Err(error) = file.read_to_string(&mut content) {
        eprintln!(
            "Warning: cannot read manifest {}: {}",
            crate::display::sanitize(&path.display().to_string()),
            crate::display::sanitize(&error.to_string())
        );
        return None;
    }
    // `toml::from_str` (document semantics) is required: since toml 1.x,
    // `str::parse::<Value>()` parses a single TOML value, not a document.
    match toml::from_str::<Value>(&content) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            eprintln!(
                "Warning: cannot parse manifest {}: {}",
                crate::display::sanitize(&path.display().to_string()),
                crate::display::sanitize(&error.to_string())
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
            eprintln!(
                "Warning: cannot inspect {}: {}",
                crate::display::sanitize(&path.display().to_string()),
                crate::display::sanitize(&error.to_string())
            );
            return None;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    Some(path.to_path_buf())
}

/// Memoized manifest read: ancestor Cargo.toml files are shared across
/// candidates, so each is parsed at most once per scan.
fn cached_manifest(path: &Path, cache: &mut HashMap<PathBuf, Option<Value>>) -> Option<Value> {
    if let Some(cached) = cache.get(path) {
        return cached.clone();
    }
    let manifest = read_manifest(path);
    cache.insert(path.to_path_buf(), manifest.clone());
    manifest
}

/// Resolve the `target/` directory a manifest builds into, returning the
/// owning root directory and the target path.
///
/// A real `target/` next to the manifest wins. Otherwise the nearest
/// ancestor workspace (bounded by the scan root) owns the build, matching
/// Cargo's nearest-workspace rule; if that workspace has no `target/`, the
/// package has no artifacts — outer workspaces are never consulted.
///
/// `CARGO_TARGET_DIR` and `build.target-dir` overrides are not honored: only
/// `target/` directly under a project or workspace root is ever cleaned.
///
/// A `target/` without a Cargo marker (see [`is_cargo_target_dir`]) is not a
/// build directory: it is reported once and treated as absent.
fn resolve_target_dir(
    project_dir: &Path,
    manifest: &Value,
    scan_root: &Path,
    cache: &mut HashMap<PathBuf, Option<Value>>,
    warned: &mut HashSet<PathBuf>,
) -> Option<(PathBuf, PathBuf)> {
    if let Some(target) = real_build_dir(&project_dir.join("target"), warned) {
        return Some((project_dir.to_path_buf(), target));
    }
    if manifest.get("workspace").is_some() {
        return None;
    }
    for ancestor in project_dir
        .ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.starts_with(scan_root))
    {
        let Some(ancestor_manifest) = cached_manifest(&ancestor.join("Cargo.toml"), cache) else {
            continue;
        };
        if ancestor_manifest.get("workspace").is_none() {
            continue;
        }
        if workspace_membership(project_dir, ancestor, &ancestor_manifest) != Some(true) {
            // A nearest workspace is an ownership boundary. Do not attribute
            // an excluded or unresolved package to an outer workspace.
            return None;
        }
        return real_build_dir(&ancestor.join("target"), warned)
            .map(|target| (ancestor.to_path_buf(), target));
    }
    None
}

/// A real `target/` directory that carries a Cargo marker; anything else is
/// warned about once per path and treated as if it did not exist.
fn real_build_dir(path: &Path, warned: &mut HashSet<PathBuf>) -> Option<PathBuf> {
    let target = real_directory(path)?;
    if is_cargo_target_dir(&target) {
        return Some(target);
    }
    if warned.insert(target.clone()) {
        eprintln!(
            "Warning: skipping {}: not a Cargo build directory (no CACHEDIR.TAG or .rustc_info.json)",
            crate::display::sanitize(&target.display().to_string())
        );
    }
    None
}

/// Whether `path` is a Cargo build directory: a real (non-symlink) directory
/// holding a regular `CACHEDIR.TAG` whose contents start with Cargo's
/// signature, or a regular non-symlink `.rustc_info.json`.
///
/// This is the fail-closed gate that keeps a folder merely named `target`
/// (with unrelated contents) from ever being removed.
pub fn is_cargo_target_dir(path: &Path) -> bool {
    let target_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        return false;
    }
    let cachedir = path.join("CACHEDIR.TAG");
    if let Ok(file) = open_regular_file_nofollow(&cachedir) {
        let mut prefix = Vec::with_capacity(CACHEDIR_SIGNATURE.len());
        let _ = file
            .take(CACHEDIR_SIGNATURE.len() as u64)
            .read_to_end(&mut prefix);
        if prefix.starts_with(CACHEDIR_SIGNATURE) {
            return true;
        }
    }
    open_regular_file_nofollow(&path.join(RUSTC_INFO_FILE)).is_ok()
}

/// Nearest manifest at or above `project_dir` declaring `[workspace]`,
/// bounded by the scan root.
fn find_workspace_manifest(
    project_dir: &Path,
    scan_root: &Path,
    cache: &mut HashMap<PathBuf, Option<Value>>,
) -> Option<Value> {
    for ancestor in project_dir
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(scan_root))
    {
        let Some(manifest) = cached_manifest(&ancestor.join("Cargo.toml"), cache) else {
            continue;
        };
        if manifest.get("workspace").is_some() {
            return (workspace_membership(project_dir, ancestor, &manifest) == Some(true))
                .then_some(manifest);
        }
    }
    None
}

/// Return whether `project_dir` is declared as a member of this workspace.
/// Unsupported or malformed member patterns return `None` so callers fail
/// closed instead of borrowing an outer workspace target.
fn workspace_membership(
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Value,
) -> Option<bool> {
    let relative = project_dir.strip_prefix(workspace_root).ok()?;
    let members = workspace_patterns(manifest, "members")?;
    let excludes = workspace_patterns(manifest, "exclude")?;
    let excluded = excludes
        .iter()
        .any(|pattern| path_matches_pattern(relative, pattern));
    if excluded {
        return Some(false);
    }
    if relative.as_os_str().is_empty() {
        return Some(manifest.get("package").is_some());
    }
    Some(
        members
            .iter()
            .any(|pattern| path_matches_pattern(relative, pattern)),
    )
}

/// Read Cargo workspace path patterns, rejecting non-string entries and
/// parent/absolute paths. `members` absent means only the root package is a
/// member; a virtual workspace must list its members explicitly.
fn workspace_patterns(manifest: &Value, key: &str) -> Option<Vec<String>> {
    let Some(value) = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get(key))
    else {
        return Some(Vec::new());
    };
    let values = value.as_array()?;
    values
        .iter()
        .map(|value| {
            let pattern = value.as_str()?;
            let path = Path::new(pattern);
            if path.is_absolute()
                || path.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
                || pattern
                    .chars()
                    .any(|character| matches!(character, '[' | ']' | '{' | '}' | '\\'))
            {
                return None;
            }
            Some(pattern.to_string())
        })
        .collect()
}

fn path_matches_pattern(path: &Path, pattern: &str) -> bool {
    let path_components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let pattern_components: Vec<String> = Path::new(pattern)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    path_components.len() == pattern_components.len()
        && path_components
            .iter()
            .zip(pattern_components.iter())
            .all(|(name, pattern)| wildcard_component_matches(pattern, name))
}

/// Match Cargo's common `*` and `?` component patterns. Character classes,
/// braces, and escapes are rejected by member enumeration below; this helper
/// returns false for those forms so they cannot accidentally broaden scope.
fn wildcard_component_matches(pattern: &str, value: &str) -> bool {
    if pattern
        .chars()
        .any(|character| matches!(character, '[' | ']' | '{' | '}' | '\\'))
    {
        return false;
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut matches = vec![false; value.len() + 1];
    matches[0] = true;
    for item in pattern {
        let mut next = vec![false; value.len() + 1];
        match item {
            '*' => {
                let mut prefix = false;
                for index in 0..=value.len() {
                    prefix |= matches[index];
                    next[index] = prefix;
                }
            }
            '?' => {
                for index in 0..value.len() {
                    next[index + 1] |= matches[index];
                }
            }
            literal => {
                for index in 0..value.len() {
                    if value[index] == literal {
                        next[index + 1] |= matches[index];
                    }
                }
            }
        }
        matches = next;
    }
    matches[value.len()]
}

/// Expand declared workspace members, including hidden directories skipped by
/// the general scanner. Any unreadable, symlinked, malformed, or unsupported
/// member makes the result indeterminate so ignored-project protection can
/// fail closed for the shared target.
fn resolve_workspace_members(
    workspace_root: &Path,
    workspace_manifest: &Value,
    cache: &mut HashMap<PathBuf, Option<Value>>,
) -> Result<Vec<(PathBuf, Value)>, String> {
    let member_patterns = workspace_patterns(workspace_manifest, "members")
        .ok_or_else(|| "workspace members contain an unsupported path pattern".to_string())?;
    let exclude_patterns = workspace_patterns(workspace_manifest, "exclude")
        .ok_or_else(|| "workspace excludes contain an unsupported path pattern".to_string())?;
    let mut members = Vec::new();
    for pattern in member_patterns {
        let found = expand_workspace_pattern(workspace_root, &pattern, true)?;
        if found.is_empty() {
            return Err(format!(
                "workspace member pattern {pattern:?} matched no directories"
            ));
        }
        members.extend(found);
    }
    let mut excluded = HashSet::new();
    for pattern in exclude_patterns {
        excluded.extend(expand_workspace_pattern(workspace_root, &pattern, false)?);
    }
    members.sort();
    members.dedup();

    let mut resolved = Vec::new();
    for member_dir in members {
        if excluded.contains(&member_dir) {
            continue;
        }
        let manifest_path = member_dir.join("Cargo.toml");
        let manifest = cached_manifest(&manifest_path, cache).ok_or_else(|| {
            format!(
                "cannot resolve declared workspace member manifest {}",
                member_dir.display()
            )
        })?;
        if manifest.get("package").is_none() {
            return Err(format!(
                "declared workspace member {} has no package table",
                member_dir.display()
            ));
        }
        resolved.push((member_dir, manifest));
    }
    Ok(resolved)
}

/// Expand one component-wise workspace path pattern without following links.
/// Missing excludes are harmless; missing members are reported to the caller.
fn expand_workspace_pattern(
    workspace_root: &Path,
    pattern: &str,
    required: bool,
) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(pattern);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "workspace path pattern {pattern:?} escapes its root"
        ));
    }
    let components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let mut paths = vec![workspace_root.to_path_buf()];
    for component in components {
        if component
            .chars()
            .any(|character| matches!(character, '[' | ']' | '{' | '}' | '\\'))
        {
            return Err(format!("workspace path pattern {pattern:?} is unsupported"));
        }
        let mut next = Vec::new();
        for parent in &paths {
            if component.contains('*') || component.contains('?') {
                let entries = fs::read_dir(parent).map_err(|error| {
                    format!(
                        "cannot read workspace member directory {}: {error}",
                        parent.display()
                    )
                })?;
                for entry in entries {
                    let entry = entry.map_err(|error| {
                        format!(
                            "cannot inspect workspace member in {}: {error}",
                            parent.display()
                        )
                    })?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        return Err(format!(
                            "workspace member directory {} contains a non-UTF-8 entry",
                            parent.display()
                        ));
                    };
                    if !wildcard_component_matches(&component, name) {
                        continue;
                    }
                    let file_type = entry.file_type().map_err(|error| {
                        format!(
                            "cannot inspect workspace member {}: {error}",
                            entry.path().display()
                        )
                    })?;
                    if file_type.is_symlink() {
                        return Err(format!(
                            "workspace member path {} is a symlink",
                            entry.path().display()
                        ));
                    }
                    if file_type.is_dir() {
                        next.push(entry.path());
                    }
                }
            } else {
                let candidate = parent.join(&component);
                match fs::symlink_metadata(&candidate) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!(
                            "workspace member path {} is a symlink",
                            candidate.display()
                        ))
                    }
                    Ok(metadata) if metadata.is_dir() => next.push(candidate),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound && !required => {}
                    Err(error) => {
                        return Err(format!(
                            "cannot inspect workspace member path {}: {error}",
                            candidate.display()
                        ))
                    }
                }
            }
        }
        paths = next;
        if paths.is_empty() {
            break;
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn ignored_name(name: &str, ignored_projects: &[String]) -> bool {
    ignored_projects
        .iter()
        .any(|ignored| ignored.trim().eq_ignore_ascii_case(name.trim()))
}

/// Include workspace members that the normal walk intentionally skips, such
/// as hidden packages. Also carry workspace ignore protection across a member
/// that has an old local target, since cleaning the shared target would still
/// discard that member's shared build outputs.
fn augment_workspace_members(
    groups: &mut BTreeMap<PathBuf, TargetGroup>,
    workspaces: &[(PathBuf, Value)],
    manifest_cache: &mut HashMap<PathBuf, Option<Value>>,
    warned: &mut HashSet<PathBuf>,
    ignored_projects: &[String],
) {
    for (workspace_root, workspace_manifest) in workspaces {
        let Some(workspace_target) = real_build_dir(&workspace_root.join("target"), warned) else {
            continue;
        };
        let workspace_key = match dunce::canonicalize(&workspace_target) {
            Ok(path) => path,
            Err(error) => {
                eprintln!(
                    "Warning: cannot resolve workspace target {}: {}",
                    crate::display::sanitize(&workspace_target.display().to_string()),
                    crate::display::sanitize(&error.to_string())
                );
                continue;
            }
        };
        if !groups.contains_key(&workspace_key) {
            continue;
        }
        let resolved =
            match resolve_workspace_members(workspace_root, workspace_manifest, manifest_cache) {
                Ok(members) => members,
                Err(error) => {
                    if !ignored_projects.is_empty() {
                        if let Some(group) = groups.get_mut(&workspace_key) {
                            group.unresolved_workspace_member = true;
                        }
                    }
                    eprintln!(
                        "Warning: cannot certify workspace members for {}: {}",
                        crate::display::sanitize(&workspace_root.display().to_string()),
                        crate::display::sanitize(&error)
                    );
                    continue;
                }
            };
        for (member_dir, member_manifest) in resolved {
            let name = extract_project_name(&member_manifest, &member_dir);
            let kind = detect_project_kind(&member_manifest, Some(workspace_manifest));
            if ignored_name(&name, ignored_projects) {
                if let Some(group) = groups.get_mut(&workspace_key) {
                    group.ignored_workspace_member = true;
                }
            }

            let member_target = real_build_dir(&member_dir.join("target"), warned)
                .unwrap_or_else(|| workspace_target.clone());
            let target_key = match dunce::canonicalize(&member_target) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "Warning: cannot resolve member target {}: {}",
                        crate::display::sanitize(&member_target.display().to_string()),
                        crate::display::sanitize(&error.to_string())
                    );
                    continue;
                }
            };
            let target_group = groups.entry(target_key).or_insert_with(|| TargetGroup {
                owner: if member_target == workspace_target {
                    workspace_root.clone()
                } else {
                    member_dir.clone()
                },
                target: member_target,
                members: Vec::new(),
                ignored_workspace_member: false,
                unresolved_workspace_member: false,
            });
            target_group.members.push((name, kind));
        }
    }
}

/// Revalidate that a cleaner-supplied root is the Cargo project or workspace
/// the scan reported: a real directory whose manifest is a package named
/// `expected_name`, or a workspace whose directory is named `expected_name`.
pub fn validate_project_root(path: &Path, expected_name: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("project root is not a real directory".to_string());
    }
    let manifest = read_manifest(&path.join("Cargo.toml"))
        .ok_or_else(|| "project root has no valid Cargo.toml".to_string())?;
    if manifest.get("package").is_none() && manifest.get("workspace").is_none() {
        return Err("project root is not a Cargo package or workspace".to_string());
    }
    let identity =
        package_name(&manifest).or_else(|| manifest.get("workspace").map(|_| directory_name(path)));
    if identity.as_deref() == Some(expected_name) {
        return Ok(());
    }
    Err(format!(
        "project identity does not match Cargo project {expected_name}"
    ))
}

/// Dependency tables (`[dependencies]`, `[build-dependencies]`, and their
/// `[target.*]` variants) of a manifest. `[dev-dependencies]` are excluded:
/// a Tauri crate used only by tests does not make a Tauri app.
fn dependency_tables(manifest: &Value) -> Vec<&toml::map::Map<String, Value>> {
    let mut tables = Vec::new();
    for key in ["dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(key).and_then(Value::as_table) {
            tables.push(table);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            for key in ["dependencies", "build-dependencies"] {
                if let Some(table) = target.get(key).and_then(Value::as_table) {
                    tables.push(table);
                }
            }
        }
    }
    tables
}

fn detect_project_kind(manifest: &Value, workspace_manifest: Option<&Value>) -> ProjectKind {
    // `optional = true` still counts (feature-gated Tauri app). Only the
    // exact `tauri` / `tauri-build` crates (by key or `package`) count;
    // `tauri-plugin-*` alone does not.
    let member_tables = dependency_tables(manifest);
    let mut tables = member_tables.clone();
    if let Some(table) = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Value::as_table)
    {
        tables.push(table);
    }
    if tables
        .iter()
        .flat_map(|table| table.iter())
        .any(|(name, value)| is_tauri_dep_entry(name, value))
    {
        return ProjectKind::TauriApp;
    }

    // Members may inherit (possibly renamed) dependencies from
    // `[workspace.dependencies]` via `name.workspace = true`.
    let workspace_dependencies = workspace_manifest
        .and_then(|manifest| manifest.get("workspace"))
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Value::as_table);
    if let Some(workspace_dependencies) = workspace_dependencies {
        let inherited = member_tables
            .iter()
            .flat_map(|table| table.iter())
            .any(|(name, value)| {
                value
                    .get("workspace")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && workspace_dependencies
                        .get(name)
                        .is_some_and(|ws_value| is_tauri_dep_entry(name, ws_value))
            });
        if inherited {
            return ProjectKind::TauriApp;
        }
    }
    ProjectKind::RustProject
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

fn package_name(manifest: &Value) -> Option<String> {
    manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn directory_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Extract the project name from a parsed Cargo manifest.
fn extract_project_name(manifest: &Value, project_dir: &Path) -> String {
    package_name(manifest).unwrap_or_else(|| directory_name(project_dir))
}

/// Path components of a target-relative directory, used to classify files
/// into the same buckets the clean modes remove.
fn classify(parent: &Path) -> (bool, bool, bool, bool) {
    let components: Vec<&OsStr> = parent.iter().collect();
    let profile = |name: &str| {
        components.first() == Some(&OsStr::new(name))
            || (components.len() >= 2
                && components[0] != OsStr::new("debug")
                && components[0] != OsStr::new("release")
                && components[1] == OsStr::new(name))
    };
    let within_depth = |name: &str| components.iter().take(3).any(|c| *c == OsStr::new(name));
    (
        profile("debug"),
        profile("release"),
        within_depth("incremental"),
        within_depth("deps"),
    )
}

/// Analyze a target/ directory in a single traversal, computing total size,
/// breakdown (including target triples), and newest modification time.
///
/// Sizes are apparent sizes (`st_size`) of regular files; symlinks count as
/// zero. Hardlinks are counted once per link, and sparse files / clones may
/// differ from actual reclaimed disk space.
pub fn analyze_target(
    target_dir: &Path,
) -> Result<(u64, TargetBreakdown, Option<SystemTime>), ScanError> {
    let mut breakdown = TargetBreakdown::default();
    let mut total_size = 0u64;
    let mut last_modified: Option<SystemTime> = None;

    for result in WalkDir::new(target_dir)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
    {
        let entry = result.map_err(|error| ScanError::Traversal(error.to_string()))?;
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
            last_modified = Some(last_modified.map_or(mtime, |prev| prev.max(mtime)));
        }

        let parent = entry
            .path()
            .strip_prefix(target_dir)
            .ok()
            .and_then(Path::parent)
            .unwrap_or(Path::new(""));
        let (is_debug, is_release, is_incremental, is_deps) = classify(parent);
        if is_debug {
            breakdown.debug_size = breakdown.debug_size.saturating_add(len);
        } else if is_release {
            breakdown.release_size = breakdown.release_size.saturating_add(len);
        } else {
            breakdown.other_size = breakdown.other_size.saturating_add(len);
        }
        if is_incremental {
            breakdown.incremental_size = breakdown.incremental_size.saturating_add(len);
        }
        if is_deps {
            breakdown.deps_size = breakdown.deps_size.saturating_add(len);
        }
    }

    Ok((total_size, breakdown, last_modified))
}

/// Find the root a path belongs to for `inspect`: the nearest enclosing
/// workspace if one exists, else the nearest directory with a Cargo.toml.
/// Read-only; walks ancestors of `path` only.
pub fn enclosing_project_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    let mut nearest_package: Option<PathBuf> = None;
    for ancestor in start.ancestors() {
        let Some(manifest) = read_manifest(&ancestor.join("Cargo.toml")) else {
            continue;
        };
        if manifest.get("workspace").is_some() {
            return Some(ancestor.to_path_buf());
        }
        if nearest_package.is_none() && manifest.get("package").is_some() {
            nearest_package = Some(ancestor.to_path_buf());
            if real_directory(&ancestor.join("target")).is_some() {
                return nearest_package;
            }
        }
    }
    nearest_package
}

/// Check if a walkdir entry is a hidden directory/file.
fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    // Byte-based so non-UTF8 names are still detected as hidden.
    let bytes = entry.file_name().as_encoded_bytes();
    bytes.starts_with(b".") && bytes != b"."
}

/// Check if a walkdir entry is an artifact directory we should not recurse
/// into: `node_modules`, or a `target` directory that is a Cargo build
/// output (next to a Cargo.toml, or tagged with Cargo's `CACHEDIR.TAG`).
/// A crate that merely lives in a folder named `target` is still scanned.
fn is_artifact_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name();
    if name == OsStr::new("node_modules") {
        return true;
    }
    if name != OsStr::new("target") {
        return false;
    }
    let path = entry.path();
    is_cargo_target_dir(path)
        || path
            .parent()
            .is_some_and(|parent| open_regular_file_nofollow(&parent.join("Cargo.toml")).is_ok())
}
