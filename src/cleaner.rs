use crate::config::CleanBehavior;
use crate::project::DiscoveredProject;
use crate::scanner;
use std::ffi::OsStr;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// Cargo's per-profile build lock file name.
const CARGO_LOCK_FILE: &str = ".cargo-lock";

/// Files that mark a directory as a Cargo build directory
/// (see `scanner::is_cargo_target_dir`).
const CARGO_MARKERS: [&str; 2] = ["CACHEDIR.TAG", ".rustc_info.json"];

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
    verified: VerifiedTarget,
}

/// Handles kept open from validation through measurement and mutation. Unix
/// operations are relative to these directory descriptors; Windows handles
/// deny delete sharing on every ancestor so the path chain cannot be renamed
/// during the operation.
#[derive(Debug)]
struct VerifiedTarget {
    target_path: PathBuf,
    project: File,
    target_dir: File,
    _ancestors: Vec<File>,
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
    let verified = open_verified_target(project)?;
    #[cfg(unix)]
    validate_target_mount_boundaries(&verified.target_dir).map_err(|error| {
        format!(
            "cannot safely inspect target mount boundaries for {}: {error}",
            verified.target_path.display()
        )
    })?;
    let target = verified.target_path.clone();
    let mut paths = match mode {
        CleanMode::Full if options.keep_bundles => {
            // Keep Cargo's markers with the bundles so the remaining
            // `target/` still passes `scanner::is_cargo_target_dir`.
            let mut preserved = bundle_dirs(&target);
            preserved.extend(CARGO_MARKERS.iter().map(|name| target.join(name)));
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
        verified,
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

fn open_verified_target(project: &DiscoveredProject) -> Result<VerifiedTarget, String> {
    if !project.path.is_absolute() {
        return Err(format!(
            "project root {} is not an absolute path",
            project.path.display()
        ));
    }
    if project.artifact_dir != project.path.join("target")
        || project.artifact_dir.file_name() != Some(OsStr::new("target"))
    {
        return Err(format!(
            "artifact path {} is not target/ directly under project root {}",
            project.artifact_dir.display(),
            project.path.display()
        ));
    }

    #[cfg(unix)]
    {
        let mut chain = open_unix_directory_chain(&project.path).map_err(|error| {
            format!(
                "cannot open project root {} without following links: {error}",
                project.path.display()
            )
        })?;
        let project_dir = chain
            .pop()
            .ok_or_else(|| "project root path has no directory components".to_string())?;
        validate_project_manifest_at(&project_dir, &project.path, &project.name)?;
        let target_dir = open_unix_directory_no_mount_crossing(&project_dir, OsStr::new("target"))
            .map_err(|error| {
                format!("cannot open target directory without following links: {error}")
            })?;
        if !unix_target_has_marker(&target_dir)
            .map_err(|error| format!("cannot validate Cargo target marker: {error}"))?
        {
            return Err(format!(
                "target directory {} is not a Cargo build directory (no CACHEDIR.TAG or .rustc_info.json)",
                project.artifact_dir.display()
            ));
        }
        Ok(VerifiedTarget {
            target_path: project.artifact_dir.clone(),
            project: project_dir,
            target_dir,
            _ancestors: chain,
        })
    }

    #[cfg(windows)]
    {
        let (project_dir, ancestors) =
            open_windows_directory_chain(&project.path).map_err(|error| {
                format!(
                    "cannot hold project root path stable {}: {error}",
                    project.path.display()
                )
            })?;
        scanner::validate_project_root(&project.path, &project.name)
            .map_err(|error| format!("project root failed validation: {error}"))?;
        let target_dir = open_windows_directory(&project.artifact_dir, true)
            .map_err(|error| format!("cannot hold target directory stable: {error}"))?;
        if !scanner::is_cargo_target_dir(&project.artifact_dir) {
            return Err(format!(
                "target directory {} is not a Cargo build directory (no CACHEDIR.TAG or .rustc_info.json)",
                project.artifact_dir.display()
            ));
        }
        return Ok(VerifiedTarget {
            target_path: project.artifact_dir.clone(),
            project: project_dir,
            target_dir,
            _ancestors: ancestors,
        });
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = project;
        Err("handle-anchored cleaning is unsupported on this platform".to_string())
    }
}

#[cfg(unix)]
fn open_unix_directory_chain(path: &Path) -> io::Result<Vec<File>> {
    use rustix::fs::{open, Mode, OFlags};
    #[cfg(target_os = "macos")]
    let normalized_path = normalize_macos_var_alias(path);
    #[cfg(not(target_os = "macos"))]
    let normalized_path = path.to_path_buf();

    if !normalized_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not absolute",
        ));
    }
    let root = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let mut chain = vec![File::from(root)];
    for component in normalized_path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let child = open_unix_directory_at(chain.last().unwrap(), name)?;
                chain.push(child);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "non-normal root path",
                ))
            }
        }
    }
    Ok(chain)
}

#[cfg(target_os = "macos")]
fn normalize_macos_var_alias(path: &Path) -> PathBuf {
    let var = Path::new("/var");
    let Ok(remainder) = path.strip_prefix(var) else {
        return path.to_path_buf();
    };
    if matches!(fs::read_link(var), Ok(target) if target == Path::new("private/var")) {
        Path::new("/private/var").join(remainder)
    } else {
        path.to_path_buf()
    }
}

#[cfg(unix)]
fn open_unix_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    use rustix::fs::{openat, Mode, OFlags};
    let child = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    Ok(File::from(child))
}

#[cfg(target_os = "linux")]
fn open_unix_directory_no_mount_crossing(parent: &File, name: &OsStr) -> io::Result<File> {
    use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};

    let fd = openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )
    .map_err(|error| {
        if error == rustix::io::Errno::XDEV {
            io::Error::other("refusing to cross a mounted directory boundary")
        } else if error == rustix::io::Errno::NOSYS
            || error == rustix::io::Errno::INVAL
            || error == rustix::io::Errno::OPNOTSUPP
            || error == rustix::io::Errno::PERM
        {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "Linux openat2 RESOLVE_NO_XDEV is unavailable or blocked ({error}); refusing target traversal"
                ),
            )
        } else {
            io::Error::from(error)
        }
    })?;
    Ok(File::from(fd))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_unix_directory_no_mount_crossing(parent: &File, name: &OsStr) -> io::Result<File> {
    use std::os::unix::fs::MetadataExt;

    let parent_device = parent.metadata()?.dev();
    let child = open_unix_directory_at(parent, name)?;
    if child.metadata()?.dev() != parent_device {
        return Err(io::Error::other(
            "refusing to cross a mounted directory boundary",
        ));
    }
    Ok(child)
}

#[cfg(unix)]
fn validate_target_mount_boundaries(target: &File) -> io::Result<()> {
    use rustix::fs::{statat, AtFlags, Dir, FileType};
    use std::os::unix::ffi::OsStrExt;

    let mut entries = Dir::read_from(target).map_err(io::Error::from)?;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        let name = OsStr::from_bytes(raw_name);
        let stat = statat(target, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let child = open_unix_directory_no_mount_crossing(target, name)?;
            validate_target_mount_boundaries(&child)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_project_manifest_at(
    root: &File,
    path: &Path,
    expected_name: &str,
) -> Result<(), String> {
    use rustix::fs::{openat, Mode, OFlags};
    let fd = openat(
        root,
        "Cargo.toml",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| format!("cannot open project manifest without following links: {error}"))?;
    let mut file = File::from(fd);
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("project Cargo.toml is not a regular file".to_string());
    }
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|error| format!("cannot read project manifest: {error}"))?;
    let manifest = toml::from_str::<toml::Value>(&content)
        .map_err(|error| format!("cannot parse project manifest: {error}"))?;
    if manifest.get("package").is_none() && manifest.get("workspace").is_none() {
        return Err("project root is not a Cargo package or workspace".to_string());
    }
    let identity = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .or_else(|| {
            manifest
                .get("workspace")
                .and_then(|_| path.file_name().and_then(OsStr::to_str))
        });
    if identity == Some(expected_name) {
        Ok(())
    } else {
        Err(format!(
            "project identity does not match Cargo project {expected_name}"
        ))
    }
}

#[cfg(unix)]
fn unix_target_has_marker(target: &File) -> io::Result<bool> {
    use rustix::fs::{openat, Mode, OFlags};
    for marker in CARGO_MARKERS {
        let fd = match openat(
            target,
            marker,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::NOENT || error == rustix::io::Errno::LOOP => {
                continue
            }
            Err(error) => return Err(io::Error::from(error)),
        };
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            continue;
        }
        if marker == "CACHEDIR.TAG" {
            let mut prefix = Vec::with_capacity(scanner::CACHEDIR_SIGNATURE.len());
            file.take(scanner::CACHEDIR_SIGNATURE.len() as u64)
                .read_to_end(&mut prefix)?;
            if prefix.starts_with(scanner::CACHEDIR_SIGNATURE) {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn open_windows_directory_chain(path: &Path) -> io::Result<(File, Vec<File>)> {
    let mut ancestors = Vec::new();
    let mut paths: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
    paths.reverse();
    let mut project = None;
    for ancestor in paths {
        let handle = open_windows_directory(&ancestor, false)?;
        if ancestor == path {
            project = Some(handle);
        } else {
            ancestors.push(handle);
        }
    }
    project
        .map(|project| (project, ancestors))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "project path has no ancestors"))
}

#[cfg(windows)]
fn open_windows_directory(path: &Path, delete_access: bool) -> io::Result<File> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const DELETE: u32 = 0x0001_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let access =
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | if delete_access { DELETE } else { 0 };
    let handle = OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let metadata = handle.metadata()?;
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory is a reparse point or not a directory",
        ));
    }
    Ok(handle)
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
    let canonical_target = dunce::canonicalize(target)
        .map_err(|error| format!("cannot resolve target {}: {error}", target.display()))?;
    if !canonical_parent.starts_with(&canonical_target) {
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
#[cfg(windows)]
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

fn size_of_plan(plan: &CleanPlan, path: &Path) -> Result<u64, String> {
    #[cfg(unix)]
    {
        let relative = path.strip_prefix(&plan.target).map_err(|_| {
            format!(
                "clean path {} is outside target directory {}",
                path.display(),
                plan.target.display()
            )
        })?;
        size_of_relative_at(&plan.verified.target_dir, relative)
            .map_err(|error| format!("cannot measure {}: {error}", path.display()))
    }
    #[cfg(windows)]
    {
        let _ = &plan.verified.target_dir;
        return size_of(path);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (plan, path);
        Err("handle-anchored measurement is unsupported on this platform".to_string())
    }
}

#[cfg(unix)]
fn size_of_relative_at(target: &File, relative: &Path) -> io::Result<u64> {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return size_directory_contents_at(target);
    }
    if components
        .iter()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "non-normal clean path",
        ));
    }
    let mut parent = target.try_clone()?;
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        parent = open_unix_directory_no_mount_crossing(&parent, name)?;
    }
    let Some(Component::Normal(name)) = components.last() else {
        unreachable!()
    };
    size_entry_at(&parent, name)
}

#[cfg(unix)]
fn size_directory_contents_at(directory: &File) -> io::Result<u64> {
    use rustix::fs::Dir;
    use std::os::unix::ffi::OsStrExt;
    let mut entries = Dir::read_from(directory).map_err(io::Error::from)?;
    let mut size = 0u64;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        size = size.saturating_add(size_entry_at(directory, OsStr::from_bytes(raw_name))?);
    }
    Ok(size)
}

#[cfg(unix)]
fn size_entry_at(parent: &File, name: &OsStr) -> io::Result<u64> {
    use rustix::fs::{openat, statat, AtFlags, FileType, Mode, OFlags};
    let stat = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::Symlink => Ok(0),
        FileType::Directory => {
            let directory = open_unix_directory_no_mount_crossing(parent, name)?;
            size_directory_contents_at(&directory)
        }
        FileType::RegularFile => {
            let fd = openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            let file = File::from(fd);
            let metadata = file.metadata()?;
            if metadata.is_file() {
                Ok(metadata.len())
            } else {
                Ok(0)
            }
        }
        _ => Ok(0),
    }
}

fn path_exists_with_identity(plan: &CleanPlan, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use rustix::fs::{statat, AtFlags};
        let Ok(relative) = path.strip_prefix(&plan.target) else {
            return false;
        };
        if relative.as_os_str().is_empty() {
            return true;
        }
        let mut components: Vec<_> = relative.components().collect();
        let Some(Component::Normal(leaf)) = components.pop() else {
            return false;
        };
        if components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return false;
        }
        let Ok(mut parent) = plan.verified.target_dir.try_clone() else {
            return false;
        };
        for component in components {
            let Component::Normal(name) = component else {
                unreachable!()
            };
            match open_unix_directory_no_mount_crossing(&parent, name) {
                Ok(directory) => parent = directory,
                Err(_) => return false,
            }
        }
        statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW).is_ok()
    }
    #[cfg(windows)]
    {
        let _ = &plan.verified.target_dir;
        fs::symlink_metadata(path).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (plan, path);
        false
    }
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
    #[cfg(unix)]
    let result = open_unix_directory_chain(target)
        .ok()
        .and_then(|mut chain| chain.pop())
        .map(|directory| acquire_locks_from_directory(&directory));
    #[cfg(windows)]
    let result = Some(acquire_windows_build_locks(target));
    #[cfg(not(any(unix, windows)))]
    let result: Option<Result<Option<Vec<File>>, String>> = None;
    match result {
        Some(Ok(Some(_locks))) => false,
        Some(Ok(None)) | Some(Err(_)) | None => true,
    }
}

/// Acquire all Cargo build locks under the verified target. None means a
/// build already owns one; an error means the state could not be determined.
/// Acquired file handles remain live through measurement and deletion.
fn acquire_build_locks(verified: &VerifiedTarget) -> Result<Option<Vec<File>>, String> {
    #[cfg(unix)]
    {
        acquire_locks_from_directory(&verified.target_dir)
    }
    #[cfg(windows)]
    {
        acquire_windows_build_locks(&verified.target_path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = verified;
        Err("Cargo build lock inspection is unsupported on this platform".to_string())
    }
}

#[cfg(unix)]
fn acquire_locks_from_directory(target: &File) -> Result<Option<Vec<File>>, String> {
    let mut locks = Vec::new();
    let ready = acquire_unix_locks_at(target, 0, &mut locks)
        .map_err(|error| format!("cannot determine Cargo build lock state: {error}"))?;
    Ok(ready.then_some(locks))
}

#[cfg(unix)]
fn acquire_unix_locks_at(
    directory: &File,
    depth: usize,
    locks: &mut Vec<File>,
) -> io::Result<bool> {
    use rustix::fs::{openat, statat, AtFlags, Dir, FileType, Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    let mut entries = Dir::read_from(directory).map_err(io::Error::from)?;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        let name = OsStr::from_bytes(raw_name);
        let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if name == OsStr::new(CARGO_LOCK_FILE) {
            if kind != FileType::RegularFile {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Cargo .cargo-lock is not a regular non-symlink file",
                ));
            }
            let fd = openat(
                directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "opened Cargo .cargo-lock is not a regular file",
                ));
            }
            match file.try_lock() {
                Ok(()) => locks.push(file),
                Err(fs::TryLockError::WouldBlock) => return Ok(false),
                Err(fs::TryLockError::Error(error)) => return Err(error),
            }
        } else if kind == FileType::Directory && depth < 2 {
            let child = open_unix_directory_no_mount_crossing(directory, name)?;
            if !acquire_unix_locks_at(&child, depth + 1, locks)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(windows)]
fn acquire_windows_build_locks(target: &Path) -> Result<Option<Vec<File>>, String> {
    let mut locks = Vec::new();
    for result in walkdir::WalkDir::new(target)
        .follow_links(false)
        .follow_root_links(false)
        .max_depth(3)
    {
        let entry = result.map_err(|error| format!("cannot inspect Cargo lock tree: {error}"))?;
        if entry.file_name() != CARGO_LOCK_FILE {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "Cargo lock entry {} is not a regular file",
                entry.path().display()
            ));
        }
        let file = open_windows_lock_file(entry.path())
            .map_err(|error| format!("cannot open Cargo lock entry: {error}"))?;
        match file.try_lock() {
            Ok(()) => locks.push(file),
            Err(fs::TryLockError::WouldBlock) => return Ok(None),
            Err(fs::TryLockError::Error(error)) => {
                return Err(format!("cannot acquire Cargo lock entry: {error}"))
            }
        }
    }
    Ok(Some(locks))
}

#[cfg(windows)]
fn open_windows_lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cargo lock entry is not a regular file",
        ));
    }
    Ok(file)
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
        size_of_plan(&plan, path).map(|size| total.saturating_add(size))
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
    if *behavior == CleanBehavior::Trash && !trash_backend_supported() {
        return CleanResult::Error {
            message: trash_identity_error(),
        };
    }
    #[cfg(target_os = "linux")]
    if *behavior == CleanBehavior::Trash {
        for path in &plan.paths {
            if let Err(message) = validate_unix_trash_path(&plan, path) {
                return CleanResult::Error { message };
            }
        }
    }
    let _build_locks = match acquire_build_locks(&plan.verified) {
        Ok(Some(locks)) => locks,
        Ok(None) => {
            return CleanResult::Skipped {
                reason: "build in progress (Cargo build lock is held)".to_string(),
            }
        }
        Err(message) => return CleanResult::Error { message },
    };

    let mut freed = 0u64;
    let mut errors = Vec::new();
    for path in &plan.paths {
        let size = match size_of_plan(&plan, path) {
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
        match remove_path_with_identity(&plan, path, behavior) {
            Ok(()) => freed = freed.saturating_add(size),
            Err(error) => {
                // Credit whatever a failed recursive delete did remove.
                let remaining = if path_exists_with_identity(&plan, path) {
                    size_of_plan(&plan, path).unwrap_or(size)
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
fn remove_path_with_identity(
    plan: &CleanPlan,
    path: &Path,
    behavior: &CleanBehavior,
) -> Result<(), String> {
    if *behavior == CleanBehavior::Trash {
        #[cfg(target_os = "linux")]
        {
            return trash_unix_path_with_identity(plan, path);
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Err(trash_identity_error());
        }
    }
    if path == plan.target {
        return remove_target_with_identity(plan);
    }
    let relative = path
        .strip_prefix(&plan.target)
        .map_err(|_| format!("clean path {} is outside target directory", path.display()))?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "clean path {} contains non-normal components",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        remove_relative_unix(&plan.verified.target_dir, relative)
            .map_err(|error| format!("delete failed: {error}"))
    }
    #[cfg(windows)]
    {
        return remove_relative_windows(&plan.verified.target_path, relative)
            .map_err(|error| format!("delete failed: {error}"));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = relative;
        Err("handle-anchored deletion is unsupported on this platform".to_string())
    }
}

fn trash_backend_supported() -> bool {
    cfg!(target_os = "linux")
}

fn trash_identity_error() -> String {
    #[cfg(target_os = "linux")]
    {
        "Trash cannot safely stage this artifact on the same protected filesystem; no files were moved or deleted".to_string()
    }
    #[cfg(windows)]
    {
        "Trash cannot safely bind the Windows shell move to the verified file identity; no files were moved or deleted".to_string()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        "Trash cannot preserve the original restore location while binding the native move to verified identity on this Unix platform; no files were moved or deleted".to_string()
    }
    #[cfg(not(any(unix, windows)))]
    {
        "Trash is unsupported on this platform; no files were moved or deleted".to_string()
    }
}

#[cfg(target_os = "linux")]
struct UnixTrashStage {
    name: std::ffi::OsString,
    path: PathBuf,
    directory: File,
}

#[cfg(target_os = "linux")]
fn trash_unix_path_with_identity(plan: &CleanPlan, path: &Path) -> Result<(), String> {
    use rustix::fs::{renameat_with, RenameFlags};
    use std::os::unix::fs::MetadataExt;

    ensure_private_unix_ancestry(&plan.verified)?;
    let (source_parent, source_name, source_item) = open_unix_trash_source(plan, path)?;
    let source_metadata = source_item
        .metadata()
        .map_err(|error| format!("cannot inspect verified Trash source: {error}"))?;
    if !source_metadata.is_dir() {
        return Err("Trash source is not a real directory".to_string());
    }

    let stage = create_unix_trash_stage(&plan.verified)?;
    let stage_device = match stage.directory.metadata() {
        Ok(metadata) => metadata.dev(),
        Err(error) => {
            return Err(discard_empty_unix_trash_stage(
                &plan.verified,
                &stage,
                format!("cannot inspect Trash staging directory: {error}"),
            ));
        }
    };
    if source_metadata.dev() != stage_device {
        return Err(discard_empty_unix_trash_stage(
            &plan.verified,
            &stage,
            "Trash staging is on a different filesystem; no files were moved or deleted"
                .to_string(),
        ));
    }

    if let Err(error) = renameat_with(
        &source_parent,
        &source_name,
        &stage.directory,
        &source_name,
        RenameFlags::NOREPLACE,
    ) {
        return Err(discard_empty_unix_trash_stage(
            &plan.verified,
            &stage,
            format!("cannot move verified Trash source into private staging: {error}"),
        ));
    }

    let staged_item = match open_unix_directory_at(&stage.directory, &source_name) {
        Ok(item) => item,
        Err(error) => {
            return Err(restore_unix_trash_stage(
                &plan.verified,
                &stage,
                &source_parent,
                &source_name,
                format!("cannot reopen staged Trash source: {error}"),
            ));
        }
    };
    let staged_metadata = match staged_item.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(restore_unix_trash_stage(
                &plan.verified,
                &stage,
                &source_parent,
                &source_name,
                format!("cannot inspect staged Trash source: {error}"),
            ));
        }
    };
    if source_metadata.dev() != staged_metadata.dev()
        || source_metadata.ino() != staged_metadata.ino()
    {
        return Err(restore_unix_trash_stage(
            &plan.verified,
            &stage,
            &source_parent,
            &source_name,
            "staged Trash entry did not match the verified source identity".to_string(),
        ));
    }

    let staged_path = stage.path.join(&source_name);
    if let Err(error) = trash_context().delete(&staged_path) {
        return Err(restore_unix_trash_stage(
            &plan.verified,
            &stage,
            &source_parent,
            &source_name,
            format!("native Trash move failed: {error}"),
        ));
    }

    let staged_path = stage.path.join(&source_name);
    let trash_item = match find_linux_trash_item(&stage.path, &source_name) {
        Ok(item) => item,
        Err(error) => {
            return Err(format!(
                "{error}; the item may be in Trash with its restore path set to {}; staging directory retained",
                staged_path.display()
            ));
        }
    };
    if let Err(metadata_error) = rewrite_linux_trash_restore_path(&trash_item, &staged_path, path) {
        return Err(rollback_linux_trash_move(
            &plan.verified,
            &stage,
            &source_parent,
            &source_name,
            &trash_item,
            &staged_path,
            path,
            metadata_error,
        ));
    }
    rustix::fs::unlinkat(
        &plan.verified.project,
        &stage.name,
        rustix::fs::AtFlags::REMOVEDIR,
    )
    .map_err(|error| {
        format!(
            "Trash succeeded but staging cleanup failed; restore path is preserved; remove empty staging directory {} ({error})",
            stage.path.display()
        )
    })?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn ensure_private_unix_ancestry(verified: &VerifiedTarget) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    for directory in verified
        ._ancestors
        .iter()
        .chain(std::iter::once(&verified.project))
    {
        let metadata = directory
            .metadata()
            .map_err(|error| format!("cannot inspect Trash staging ancestor: {error}"))?;
        let mode = metadata.mode();
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err(
                "Trash staging refused because an ancestor is writable by other users; no files were moved or deleted"
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_unix_trash_path(plan: &CleanPlan, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    ensure_private_unix_ancestry(&plan.verified)?;
    let (_, _, source) = open_unix_trash_source(plan, path)?;
    let source_metadata = source
        .metadata()
        .map_err(|error| format!("cannot inspect verified Trash source: {error}"))?;
    if !source_metadata.is_dir() {
        return Err("Trash source is not a real directory".to_string());
    }
    let project_device = plan
        .verified
        .project
        .metadata()
        .map_err(|error| format!("cannot inspect Trash staging filesystem: {error}"))?
        .dev();
    if source_metadata.dev() != project_device {
        return Err(
            "Trash staging is on a different filesystem; no files were moved or deleted"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_unix_trash_source(
    plan: &CleanPlan,
    path: &Path,
) -> Result<(File, std::ffi::OsString, File), String> {
    if path == plan.target {
        return Ok((
            plan.verified
                .project
                .try_clone()
                .map_err(|error| error.to_string())?,
            std::ffi::OsString::from("target"),
            plan.verified
                .target_dir
                .try_clone()
                .map_err(|error| error.to_string())?,
        ));
    }

    let relative = path
        .strip_prefix(&plan.target)
        .map_err(|_| format!("Trash path {} is outside target", path.display()))?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Trash path has invalid relative components".to_string());
    }
    let mut parent = plan
        .verified
        .target_dir
        .try_clone()
        .map_err(|error| error.to_string())?;
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        parent = open_unix_directory_no_mount_crossing(&parent, name)
            .map_err(|error| format!("cannot open Trash source parent: {error}"))?;
    }
    let Some(Component::Normal(name)) = components.last() else {
        unreachable!()
    };
    let source = open_unix_directory_no_mount_crossing(&parent, name)
        .map_err(|error| format!("cannot open Trash source without following links: {error}"))?;
    Ok((parent, name.to_os_string(), source))
}

#[cfg(target_os = "linux")]
fn create_unix_trash_stage(verified: &VerifiedTarget) -> Result<UnixTrashStage, String> {
    use rustix::fs::{mkdirat, Mode};

    let project_path = verified
        .target_path
        .parent()
        .ok_or_else(|| "verified project has no parent path".to_string())?;
    let mut entropy = File::open("/dev/urandom")
        .map_err(|error| format!("cannot open secure Trash staging randomness: {error}"))?;
    for _ in 0..16 {
        let mut random = [0u8; 16];
        entropy
            .read_exact(&mut random)
            .map_err(|error| format!("cannot read secure Trash staging randomness: {error}"))?;
        let name = std::ffi::OsString::from(format!(".deoxidizer-trash-{}", hex::encode(random)));
        match mkdirat(&verified.project, &name, Mode::from_bits_truncate(0o700)) {
            Ok(()) => {
                let directory = match open_unix_directory_at(&verified.project, &name) {
                    Ok(directory) => directory,
                    Err(error) => {
                        return Err(match rustix::fs::unlinkat(
                                &verified.project,
                                &name,
                                rustix::fs::AtFlags::REMOVEDIR,
                            ) {
                                Ok(()) => format!(
                                    "cannot open private Trash staging directory: {error}"
                                ),
                                Err(cleanup_error) => format!(
                                    "cannot open private Trash staging directory: {error}; empty staging directory {} remains ({cleanup_error})",
                                    project_path.join(&name).display()
                                ),
                            });
                    }
                };
                return Ok(UnixTrashStage {
                    path: project_path.join(&name),
                    name,
                    directory,
                });
            }
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create private Trash staging directory: {error}"
                ));
            }
        }
    }
    Err("cannot allocate a unique Trash staging directory".to_string())
}

#[cfg(target_os = "linux")]
fn discard_empty_unix_trash_stage(
    verified: &VerifiedTarget,
    stage: &UnixTrashStage,
    reason: String,
) -> String {
    match rustix::fs::unlinkat(
        &verified.project,
        &stage.name,
        rustix::fs::AtFlags::REMOVEDIR,
    ) {
        Ok(()) => reason,
        Err(error) => format!(
            "{reason}; empty staging directory {} remains ({error})",
            stage.path.display()
        ),
    }
}

#[cfg(target_os = "linux")]
fn restore_unix_trash_stage(
    verified: &VerifiedTarget,
    stage: &UnixTrashStage,
    source_parent: &File,
    source_name: &OsStr,
    reason: String,
) -> String {
    use rustix::fs::{renameat_with, AtFlags, RenameFlags};
    match renameat_with(
        &stage.directory,
        source_name,
        source_parent,
        source_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            let cleanup = rustix::fs::unlinkat(&verified.project, &stage.name, AtFlags::REMOVEDIR);
            match cleanup {
                Ok(()) => format!("{reason}; original artifact was restored"),
                Err(error) => format!(
                    "{reason}; original artifact was restored but staging directory {} remains: {error}",
                    stage.path.display()
                ),
            }
        }
        Err(error) => format!(
            "{reason}; could not restore the staged artifact; recover it at {} ({error})",
            stage.path.join(source_name).display()
        ),
    }
}

#[cfg(target_os = "linux")]
fn find_linux_trash_item(
    stage_path: &Path,
    staged_name: &OsStr,
) -> Result<trash::TrashItem, String> {
    let matching: Vec<_> = trash::os_limited::list()
        .map_err(|error| format!("Trash succeeded but its item cannot be inspected: {error}"))?
        .into_iter()
        .filter(|item| item.original_parent == stage_path && item.name == staged_name)
        .collect();
    if matching.len() != 1 {
        return Err(format!(
            "Trash succeeded but its restore record is not uniquely identifiable ({} matches)",
            matching.len()
        ));
    }
    Ok(matching.into_iter().next().unwrap())
}

#[cfg(target_os = "linux")]
fn rewrite_linux_trash_restore_path(
    item: &trash::TrashItem,
    current_path: &Path,
    new_path: &Path,
) -> Result<(), String> {
    use rustix::fs::{openat, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;

    let info_path = PathBuf::from(item.id.clone());
    if !info_path.is_absolute() || info_path.file_name().is_none() {
        return Err("Trash returned a non-absolute restore record identifier".to_string());
    }
    let info_name = info_path
        .file_name()
        .ok_or_else(|| "Trash restore record has no file name".to_string())?;
    let info_parent_path = info_path
        .parent()
        .ok_or_else(|| "Trash restore record has no parent directory".to_string())?;
    let mut info_chain = open_unix_directory_chain(info_parent_path)
        .map_err(|error| format!("cannot safely open Trash restore directory: {error}"))?;
    let info_parent = info_chain
        .pop()
        .ok_or_else(|| "Trash restore directory has no handle".to_string())?;
    let fd = openat(
        &info_parent,
        info_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| format!("cannot safely open Trash restore record: {error}"))?;
    let mut info_file = File::from(fd);
    let original_info_metadata = info_file
        .metadata()
        .map_err(|error| format!("cannot inspect Trash restore record: {error}"))?;
    if !original_info_metadata.is_file() {
        return Err("Trash restore record is not a regular file".to_string());
    }
    let mut content = String::new();
    info_file
        .read_to_string(&mut content)
        .map_err(|error| format!("cannot read Trash restore record: {error}"))?;

    let expected_line = format!("Path={}", encode_freedesktop_path(current_path));
    let new_line = format!("Path={}\n", encode_freedesktop_path(new_path));
    let mut replaced = false;
    let mut updated = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        if !replaced && line.starts_with("Path=") {
            if line.trim_end_matches(['\r', '\n']) != expected_line {
                return Err(
                    "Trash restore record no longer points to the staged artifact".to_string(),
                );
            }
            if expected_line == format!("Path={}", encode_freedesktop_path(new_path)) {
                return Ok(());
            }
            updated.push_str(&new_line);
            replaced = true;
        } else {
            updated.push_str(line);
        }
    }
    if !replaced {
        return Err("Trash restore record has no Path entry".to_string());
    }

    let (temporary_name, _temporary_file) =
        create_linux_trash_info_temp(&info_parent, updated.as_bytes())?;
    use rustix::fs::{renameat_with, unlinkat, AtFlags, RenameFlags};
    if let Err(error) = renameat_with(
        &info_parent,
        &temporary_name,
        &info_parent,
        info_name,
        RenameFlags::EXCHANGE,
    ) {
        let cleanup = unlinkat(&info_parent, &temporary_name, AtFlags::empty());
        return Err(match cleanup {
            Ok(()) => format!("cannot atomically install Trash restore path: {error}"),
            Err(cleanup_error) => format!(
                "cannot atomically install Trash restore path: {error}; temporary restore record remains at {} ({cleanup_error})",
                info_parent_path.join(&temporary_name).display()
            ),
        });
    }

    let exchanged = openat(
        &info_parent,
        &temporary_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from);
    let old_record_matches = exchanged
        .ok()
        .and_then(|file| file.metadata().ok())
        .is_some_and(|metadata| {
            metadata.dev() == original_info_metadata.dev()
                && metadata.ino() == original_info_metadata.ino()
        });
    if !old_record_matches {
        let rollback = renameat_with(
            &info_parent,
            &temporary_name,
            &info_parent,
            info_name,
            RenameFlags::EXCHANGE,
        );
        return Err(match rollback {
            Ok(()) => match unlinkat(&info_parent, &temporary_name, AtFlags::empty()) {
                Ok(()) => "Trash restore record changed during update; original record was restored".to_string(),
                Err(error) => format!(
                    "Trash restore record changed during update; original record was restored but temporary metadata remains at {} ({error})",
                    info_parent_path.join(&temporary_name).display()
                ),
            },
            Err(error) => format!(
                "Trash restore record changed during update; could not roll back the exchange; inspect {} and {} ({error})",
                info_path.display(),
                info_parent_path.join(&temporary_name).display()
            ),
        });
    }

    if let Err(error) = unlinkat(&info_parent, &temporary_name, AtFlags::empty()) {
        let rollback = renameat_with(
            &info_parent,
            &temporary_name,
            &info_parent,
            info_name,
            RenameFlags::EXCHANGE,
        );
        return Err(match rollback {
            Ok(()) => match unlinkat(&info_parent, &temporary_name, AtFlags::empty()) {
                Ok(()) => format!("cannot remove superseded Trash restore record: {error}; original metadata was restored"),
                Err(cleanup_error) => format!(
                    "cannot remove superseded Trash restore record: {error}; original metadata was restored but temporary metadata remains at {} ({cleanup_error})",
                    info_parent_path.join(&temporary_name).display()
                ),
            },
            Err(rollback_error) => format!(
                "cannot remove superseded Trash restore record: {error}; restore metadata may point to the original artifact; rollback failed; inspect {} and {} ({rollback_error})",
                info_path.display(),
                info_parent_path.join(&temporary_name).display()
            ),
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn create_linux_trash_info_temp(
    directory: &File,
    contents: &[u8],
) -> Result<(std::ffi::OsString, File), String> {
    use rustix::fs::{openat, unlinkat, AtFlags, Mode, OFlags};
    use std::io::Write;

    let mut entropy = File::open("/dev/urandom")
        .map_err(|error| format!("cannot open secure Trash metadata randomness: {error}"))?;
    for _ in 0..16 {
        let mut random = [0u8; 16];
        entropy
            .read_exact(&mut random)
            .map_err(|error| format!("cannot read secure Trash metadata randomness: {error}"))?;
        let name =
            std::ffi::OsString::from(format!(".deoxidizer-trash-info-{}", hex::encode(random)));
        let fd = match openat(
            directory,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create Trash metadata staging file: {error}"
                ))
            }
        };
        let mut file = File::from(fd);
        if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
            let cleanup = unlinkat(directory, &name, AtFlags::empty());
            return Err(match cleanup {
                Ok(()) => format!("cannot write staged Trash restore record: {error}"),
                Err(cleanup_error) => format!(
                    "cannot write staged Trash restore record: {error}; temporary file remains ({cleanup_error})"
                ),
            });
        }
        return Ok((name, file));
    }
    Err("cannot allocate a unique Trash metadata staging file".to_string())
}

#[cfg(target_os = "linux")]
fn rollback_linux_trash_move(
    verified: &VerifiedTarget,
    stage: &UnixTrashStage,
    source_parent: &File,
    source_name: &OsStr,
    item: &trash::TrashItem,
    staged_path: &Path,
    original_path: &Path,
    metadata_error: String,
) -> String {
    let stage_metadata = rewrite_linux_trash_restore_path(item, original_path, staged_path)
        .or_else(|_| rewrite_linux_trash_restore_path(item, staged_path, staged_path));
    if let Err(error) = stage_metadata {
        return format!(
            "cannot preserve original Trash restore path ({metadata_error}); cannot reset restore record to private staging path ({error}); item remains in Trash; staging directory retained at {}",
            stage.path.display()
        );
    }

    let mut restore_item = item.clone();
    restore_item.original_parent = stage.path.clone();
    restore_item.name = source_name.to_os_string();
    if let Err(error) = trash::os_limited::restore_all([restore_item]) {
        return format!(
            "cannot preserve original Trash restore path ({metadata_error}); cannot restore item from Trash ({error}); recover it at {}",
            staged_path.display()
        );
    }
    match open_unix_directory_at(&stage.directory, source_name) {
        Ok(restored) if restored.metadata().is_ok_and(|metadata| metadata.is_dir()) => {
            let reason = format!(
                "cannot preserve original Trash restore path ({metadata_error}); item was restored from Trash"
            );
            restore_unix_trash_stage(verified, stage, source_parent, source_name, reason)
        }
        Ok(_) => format!(
            "cannot preserve original Trash restore path ({metadata_error}); restored item is not a real directory; recover it at {}",
            staged_path.display()
        ),
        Err(error) => format!(
            "cannot preserve original Trash restore path ({metadata_error}); restored item could not be opened safely ({error}); recover it at {}",
            staged_path.display()
        ),
    }
}

#[cfg(target_os = "linux")]
fn encode_freedesktop_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut encoded = String::new();
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(target_os = "linux")]
fn trash_context() -> trash::TrashContext {
    trash::TrashContext::default()
}

#[cfg(unix)]
fn remove_target_with_identity(plan: &CleanPlan) -> Result<(), String> {
    use rustix::fs::{unlinkat, AtFlags};
    remove_directory_contents_at(&plan.verified.target_dir, &CARGO_MARKERS)
        .map_err(|error| format!("delete failed before marker removal: {error}"))?;
    for marker in CARGO_MARKERS {
        match remove_entry_at(&plan.verified.target_dir, OsStr::new(marker)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("delete failed removing Cargo marker: {error}"));
            }
        }
    }
    unlinkat(
        &plan.verified.project,
        OsStr::new("target"),
        AtFlags::REMOVEDIR,
    )
    .map_err(|error| format!("delete failed removing target directory: {error}"))?;
    Ok(())
}

#[cfg(unix)]
fn remove_relative_unix(target: &File, relative: &Path) -> io::Result<()> {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid relative clean path",
        ));
    }
    let mut parent = target.try_clone()?;
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        parent = open_unix_directory_no_mount_crossing(&parent, name)?;
    }
    let Some(Component::Normal(name)) = components.last() else {
        unreachable!()
    };
    remove_entry_at(&parent, name)
}

#[cfg(unix)]
fn remove_entry_at(parent: &File, name: &OsStr) -> io::Result<()> {
    use rustix::fs::{statat, unlinkat, AtFlags, FileType};
    let stat = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
        let directory = open_unix_directory_no_mount_crossing(parent, name)?;
        remove_directory_contents_at(&directory, &[])?;
        unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)
    } else {
        unlinkat(parent, name, AtFlags::empty()).map_err(io::Error::from)
    }
}

#[cfg(unix)]
fn remove_directory_contents_at(directory: &File, preserved: &[&str]) -> io::Result<()> {
    use rustix::fs::Dir;
    use std::os::unix::ffi::OsStrExt;
    let mut entries = Dir::read_from(directory).map_err(io::Error::from)?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let name = OsStr::from_bytes(bytes);
        if preserved.iter().any(|item| name == OsStr::new(item)) {
            continue;
        }
        names.push(name.to_os_string());
    }
    for name in names {
        remove_entry_at(directory, &name)?;
    }
    Ok(())
}

/// Trash context; on macOS uses `NSFileManager` instead of scripting
/// Finder, which avoids the Automation permission prompt and works headless.
#[cfg(windows)]
fn remove_target_with_identity(plan: &CleanPlan) -> Result<(), String> {
    remove_windows_directory_contents(&plan.verified.target_path, &CARGO_MARKERS)
        .map_err(|error| format!("delete failed before marker removal: {error}"))?;
    for marker in CARGO_MARKERS {
        let path = plan.verified.target_path.join(marker);
        if fs::symlink_metadata(&path).is_ok() {
            remove_windows_entry(&path)
                .map_err(|error| format!("delete failed removing Cargo marker: {error}"))?;
        }
    }
    delete_windows_handle(&plan.verified.target_dir)
        .map_err(|error| format!("delete failed removing target directory: {error}"))
}

#[cfg(windows)]
fn remove_relative_windows(target: &Path, relative: &Path) -> io::Result<()> {
    let mut path = target.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid relative clean path",
        ));
    }
    let mut parent_handles = Vec::new();
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        path.push(name);
        parent_handles.push(open_windows_directory(&path, false)?);
    }
    let Some(Component::Normal(leaf)) = components.last() else {
        unreachable!()
    };
    path.push(leaf);
    remove_windows_entry(&path)
}

#[cfg(windows)]
fn remove_windows_directory_contents(path: &Path, preserved: &[&str]) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if preserved.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        remove_windows_entry(&entry.path())?;
    }
    Ok(())
}

#[cfg(windows)]
fn remove_windows_entry(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let handle = open_windows_delete_entry(path)?;
    let attributes = handle.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 && attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
    {
        for entry in fs::read_dir(path)? {
            remove_windows_entry(&entry?.path())?;
        }
    }
    delete_windows_handle(&handle)
}

#[cfg(windows)]
fn open_windows_delete_entry(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
    const DELETE: u32 = 0x0001_0000;
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(windows)]
fn delete_windows_handle(handle: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct FileDispositionInfo {
        delete_file: u8,
    }
    const FILE_DISPOSITION_INFO_CLASS: i32 = 4;
    #[link(name = "Kernel32")]
    extern "system" {
        fn SetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information_class: i32,
            information: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let info = FileDispositionInfo { delete_file: 1 };
    let result = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle() as *mut std::ffi::c_void,
            FILE_DISPOSITION_INFO_CLASS,
            (&info as *const FileDispositionInfo).cast(),
            std::mem::size_of::<FileDispositionInfo>() as u32,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
