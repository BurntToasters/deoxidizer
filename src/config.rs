use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const CONFIG_FILENAME: &str = ".deox_config";
pub const CONFIG_VERSION: u32 = 1;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    TauriOnly,
    TauriAndRust,
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

impl Scope {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "tauri-only" | "tauri" => Some(Scope::TauriOnly),
            "tauri-and-rust" | "both" | "all" => Some(Scope::TauriAndRust),
            "rust-only" | "rust" => Some(Scope::RustOnly),
            _ => None,
        }
    }
}

/// Cleaning behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanBehavior {
    Delete,
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

impl CleanBehavior {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "delete" | "rm" | "remove" => Some(CleanBehavior::Delete),
            "trash" | "recycle" | "bin" => Some(CleanBehavior::Trash),
            _ => None,
        }
    }
}

/// Default clean mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultMode {
    Full,
    DebugOnly,
    IncrementalOnly,
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

impl DefaultMode {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "full" => Some(DefaultMode::Full),
            "debug-only" | "debug" => Some(DefaultMode::DebugOnly),
            "incremental-only" | "incremental" => Some(DefaultMode::IncrementalOnly),
            "deps-only" | "deps" => Some(DefaultMode::DepsOnly),
            _ => None,
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
    /// Returns the path to the config file: ~/.deox_config
    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(CONFIG_FILENAME)
    }

    /// Load config from ~/.deox_config.
    ///
    /// Missing configuration is returned as a distinct error so callers that
    /// can safely use defaults do so explicitly, while destructive commands
    /// can refuse to proceed.
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    /// Load config, using defaults only when no config file exists.
    pub fn load_or_default() -> Result<Self, ConfigError> {
        match Self::load() {
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

        let config: Self = serde_json::from_str(&content).map_err(ConfigError::Parse)?;
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
        let path = Self::config_path();
        self.save_to(&path)
    }

    /// Save config to a specific path.
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
        Ok(())
    }

    /// Returns the projects directory as a PathBuf with safe ~ expansion.
    pub fn projects_path(&self) -> PathBuf {
        if let Some(rest) = self.projects_dir.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        } else if self.projects_dir == "~" {
            if let Some(home) = dirs::home_dir() {
                return home;
            }
        }
        PathBuf::from(&self.projects_dir)
    }
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
