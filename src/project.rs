use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// The type of project detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectKind {
    /// A Tauri application (Cargo.toml has a `tauri` dependency).
    TauriApp,
    /// A plain Rust/Cargo project.
    RustProject,
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectKind::TauriApp => write!(f, "Tauri"),
            ProjectKind::RustProject => write!(f, "Rust"),
        }
    }
}

/// Breakdown of space usage within a target/ directory.
///
/// Buckets mirror exactly what each clean mode removes, so a breakdown
/// subtotal is the byte count that mode would free:
/// - `debug_size`: files under `target/debug/` or `target/<triple>/debug/`.
/// - `release_size`: files under `target/release/` or `target/<triple>/release/`.
/// - `incremental_size` / `deps_size`: files under a directory with that name
///   at depth 1–3 inside `target/`.
/// - `other_size`: everything else (custom profiles, `doc/`, `tmp/`, ...).
#[derive(Debug, Clone, Default)]
pub struct TargetBreakdown {
    pub debug_size: u64,
    pub release_size: u64,
    pub incremental_size: u64,
    pub deps_size: u64,
    pub other_size: u64,
}

impl TargetBreakdown {
    /// Exclusive total of top-level profile sizes.
    ///
    /// Sums only `debug_size`, `release_size`, and `other_size`.
    /// `incremental_size` and `deps_size` are overlapping views into files
    /// already counted under a profile, so they are excluded to avoid
    /// double-counting.
    pub fn total(&self) -> u64 {
        self.debug_size
            .saturating_add(self.release_size)
            .saturating_add(self.other_size)
    }
}

/// A discovered project with its build artifact location.
///
/// One `DiscoveredProject` exists per Cargo `target/` directory. A workspace
/// whose members share one `target/` is a single project named after the
/// workspace root (its package name, or its directory name for a virtual
/// workspace); every package building into it is listed in `members`.
#[derive(Debug, Clone)]
pub struct DiscoveredProject {
    /// Project name (root `package.name`, or the root directory name).
    pub name: String,
    /// Path to the project (or workspace) root directory.
    pub path: PathBuf,
    /// Type of project detected (Tauri if any member is a Tauri app).
    pub kind: ProjectKind,
    /// Path to the validated Cargo `target/` directory.
    pub artifact_dir: PathBuf,
    /// Total size of the artifact directory in bytes.
    pub artifact_size: u64,
    /// Most recent modification time within the artifact directory.
    pub last_modified: Option<SystemTime>,
    /// Detailed size breakdown (Rust/Tauri projects only).
    pub breakdown: Option<TargetBreakdown>,
    /// Package names that build into this target (used by `ignored_projects`).
    pub members: Vec<String>,
}
