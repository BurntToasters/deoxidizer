use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const CONFIG_FILENAME: &str = ".deox_config";
pub const CONFIG_VERSION: u32 = 2;
const LEGACY_CONFIG_VERSION: u32 = 1;

#[derive(Debug)]
pub enum ConfigError {
    Missing(PathBuf),
    Io(io::Error),
    Parse(serde_json::Error),
    UnsupportedVersion(u32),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "configuration file not found: {}", path.display()),
            Self::Io(error) => write!(f, "configuration I/O error: {error}"),
            Self::Parse(error) => write!(f, "invalid configuration JSON: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(
                    f,
                    "unsupported configuration version {version} (expected {CONFIG_VERSION})"
                )
            }
            Self::Invalid(message) => write!(f, "invalid configuration: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            _ => None,
        }
    }
}

/// Project scanning scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    #[value(name = "tauri-only", alias = "tauri")]
    TauriOnly,
    #[value(name = "tauri-and-rust", alias = "both", alias = "all")]
    TauriAndRust,
    #[value(name = "rust-only", alias = "rust")]
    RustOnly,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::TauriOnly => write!(f, "tauri-only"),
            Scope::TauriAndRust => write!(f, "tauri-and-rust"),
            Scope::RustOnly => write!(f, "rust-only"),
        }
    }
}

/// Cleaning behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CleanBehavior {
    #[value(name = "delete", alias = "rm", alias = "remove")]
    Delete,
    #[value(name = "trash", alias = "recycle", alias = "bin")]
    Trash,
}

impl std::fmt::Display for CleanBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CleanBehavior::Delete => write!(f, "delete"),
            CleanBehavior::Trash => write!(f, "trash"),
        }
    }
}

/// Default clean mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultMode {
    #[value(name = "full")]
    Full,
    #[value(name = "debug-only", alias = "debug")]
    DebugOnly,
    #[value(name = "incremental-only", alias = "incremental")]
    IncrementalOnly,
    #[value(name = "deps-only", alias = "deps")]
    DepsOnly,
}

impl std::fmt::Display for DefaultMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultMode::Full => write!(f, "full"),
            DefaultMode::DebugOnly => write!(f, "debug-only"),
            DefaultMode::IncrementalOnly => write!(f, "incremental-only"),
            DefaultMode::DepsOnly => write!(f, "deps-only"),
        }
    }
}

/// The persisted configuration for deoxidizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version for future migrations.
    pub version: u32,
    /// Root directory to scan for projects.
    pub projects_dir: String,
    /// Which project types to include.
    pub scope: Scope,
    /// Whether to permanently delete or move to OS trash.
    pub clean_behavior: CleanBehavior,
    /// Default clean mode when none is specified.
    pub default_mode: DefaultMode,
    /// Minimum artifact size in MB to include in results (0 = show all).
    pub min_size_mb: u64,
    /// Project names to never auto-clean.
    #[serde(default)]
    pub ignored_projects: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigV1 {
    #[serde(rename = "version")]
    _version: u32,
    projects_dir: String,
    scope: Scope,
    clean_behavior: CleanBehavior,
    default_mode: DefaultMode,
    min_size_mb: u64,
    #[serde(default)]
    ignored_projects: Vec<String>,
}

impl From<ConfigV1> for Config {
    fn from(config: ConfigV1) -> Self {
        Self {
            version: CONFIG_VERSION,
            projects_dir: config.projects_dir,
            scope: config.scope,
            clean_behavior: config.clean_behavior,
            default_mode: config.default_mode,
            min_size_mb: config.min_size_mb,
            ignored_projects: config.ignored_projects,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let projects_dir = detect_default_projects_dir();
        Config {
            version: CONFIG_VERSION,
            projects_dir,
            scope: Scope::TauriOnly,
            clean_behavior: CleanBehavior::Delete,
            default_mode: DefaultMode::Full,
            min_size_mb: 0,
            ignored_projects: Vec::new(),
        }
    }
}

impl Config {
    /// Fallible config path: errors when HOME cannot be determined.
    ///
    /// Callers that mutate or require the filesystem must use this so a
    /// missing HOME surfaces as [`ConfigError`] instead of silently
    /// resolving to a relative `./.deox_config`.
    pub fn try_config_path() -> Result<PathBuf, ConfigError> {
        dirs::home_dir().map(|home| home.join(CONFIG_FILENAME)).ok_or_else(|| {
            ConfigError::Invalid(
                "could not determine home directory (HOME is unset); set HOME to locate configuration"
                    .to_string(),
            )
        })
    }

    /// Path to the config file for display purposes: `~/.deox_config`, or
    /// `./.deox_config` when the home directory is unknown. Anything that
    /// reads or writes the file uses [`Config::try_config_path`] instead.
    pub fn config_path() -> PathBuf {
        Self::try_config_path().unwrap_or_else(|_| PathBuf::from(".").join(CONFIG_FILENAME))
    }

    /// Load config from ~/.deox_config.
    ///
    /// Missing configuration is returned as a distinct error so callers that
    /// can safely use defaults do so explicitly, while destructive commands
    /// can refuse to proceed.
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::try_config_path()?;
        Self::load_from(&path)
    }

    /// Load config, using defaults only when no config file exists.
    pub fn load_or_default() -> Result<Self, ConfigError> {
        let path = Self::try_config_path()?;
        Self::load_or_default_from(&path)
    }

    /// Load config from a specific path, using defaults only when the file
    /// does not exist. Other errors (malformed JSON, unsupported version)
    /// propagate so callers never silently fall back on corrupt config.
    pub fn load_or_default_from(path: &Path) -> Result<Self, ConfigError> {
        match Self::load_from(path) {
            Ok(config) => Ok(config),
            Err(ConfigError::Missing(_)) => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing(path.to_path_buf()))
            }
            Err(error) => return Err(ConfigError::Io(error)),
        };

        let document: serde_json::Value =
            serde_json::from_str(&content).map_err(ConfigError::Parse)?;
        let version = document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| {
                ConfigError::Invalid("version must be a positive integer".to_string())
            })?;
        let config = match version {
            LEGACY_CONFIG_VERSION => serde_json::from_value::<ConfigV1>(document)
                .map(Config::from)
                .map_err(ConfigError::Parse)?,
            CONFIG_VERSION => {
                serde_json::from_value::<Self>(document).map_err(ConfigError::Parse)?
            }
            version => return Err(ConfigError::UnsupportedVersion(version)),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        if self.projects_dir.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "projects_dir must not be empty".to_string(),
            ));
        }
        if self.min_size_mb.checked_mul(1024 * 1024).is_none() {
            return Err(ConfigError::Invalid("min_size_mb is too large".to_string()));
        }
        Ok(())
    }

    /// Save config to ~/.deox_config.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::try_config_path()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.save_to(&path)
    }

    /// Save config to a specific path.
    ///
    /// Atomicity/permissions: `tempfile` creates the staging file with `0600`
    /// and `persist` renames it over the destination, preserving that mode on
    /// creation; the parent-directory fsync below only guarantees directory
    /// entry durability, not additional permission hardening.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        self.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(json.as_bytes())?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| io::Error::other(error.error))?;
        // Best-effort parent fsync so the rename survives a crash. Directory
        // fsync failures are ignored: durability hint only, not correctness.
        #[cfg(unix)]
        {
            if let Ok(dir) = fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    /// Returns the projects directory as a PathBuf with safe `~` expansion
    /// (`~`, `~/...`, and on Windows `~\\...`).
    pub fn projects_path(&self) -> PathBuf {
        expand_tilde(&self.projects_dir)
    }
}

/// Expand a leading `~` to the home directory; other paths are unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/")
            .or_else(|| cfg!(windows).then(|| path.strip_prefix("~\\")).flatten())
    };
    match (rest, dirs::home_dir()) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Normalize a user-entered projects directory for storage: `~`-prefixed
/// paths are kept verbatim (portable across machines), anything else is
/// made absolute against the current directory so it never depends on
/// where `deox` is run later.
pub fn normalize_projects_dir(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed == "~" || trimmed.starts_with("~/") || (cfg!(windows) && trimmed.starts_with("~\\"))
    {
        return trimmed.to_string();
    }
    std::path::absolute(trimmed)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| trimmed.to_string())
}

/// Try to auto-detect a sensible default projects directory.
fn detect_default_projects_dir() -> String {
    // Check common Git/GitHub directories
    if let Some(home) = dirs::home_dir() {
        for candidate in &[
            "Documents/GitHub",
            "GitHub",
            "Projects",
            "repos",
            "src",
            "dev",
            "code",
        ] {
            let path = home.join(candidate);
            if path.is_dir() {
                return path.to_string_lossy().to_string();
            }
        }
    }
    // Fall back to current directory
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string())
}
