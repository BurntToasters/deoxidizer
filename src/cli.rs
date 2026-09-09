use crate::cleaner::CleanMode;
use crate::config::{CleanBehavior, Config, ConfigError, DefaultMode, Scope};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Maximum `--min-size-mb` accepted at the CLI layer: `u64::MAX / 1 MiB`,
/// so `min_size_mb * 1024 * 1024` cannot overflow. Larger values are
/// rejected by clap (exit 2); stored configs re-check via `Config::validate`.
const MAX_MIN_SIZE_MB: u64 = u64::MAX / (1024 * 1024);
/// Maximum `--older-than` in days (100 years); bounds obvious typos while
/// `retain_older_than` still guards time-underflow (`checked_sub` pre-epoch)
/// with exit 2 (`u64` day-to-second mul cannot overflow).
/// `i64` because `value_parser!(u32)` ranges over `i64` in clap 4.6.
const MAX_OLDER_THAN_DAYS: i64 = 36_500;

#[derive(Parser)]
#[command(
    name = "deoxidizer",
    about = "Reclaim disk space from Rust & Tauri build artifacts 🧹",
    version,
    after_help = "Tip: Run 'deox setup' to configure your projects directory and preferences."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Check for updates and self-update the binary.
    #[arg(short = 'u', long = "update")]
    pub update: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the interactive setup wizard.
    Setup(SetupArgs),

    /// Scan and clean build artifacts.
    ///
    /// Cleaning is non-transactional: interrupting it (e.g. Ctrl-C) may leave
    /// some projects cleaned and others untouched. Re-run to finish.
    Clean(CleanArgs),

    /// Scan and display artifact sizes without deleting.
    Scan(ScanArgs),

    /// Manage deoxidizer configuration.
    Settings(SettingsArgs),

    /// Show detailed breakdown of a specific project's artifacts.
    Inspect(InspectArgs),
}

#[derive(Args)]
pub struct SetupArgs {
    /// Apply default settings without interactive prompts.
    #[arg(short = 'd', long = "default")]
    pub use_defaults: bool,

    /// Skip the overwrite confirmation when `--default` would replace an
    /// existing config file.
    #[arg(short = 'y', long = "yes", requires = "use_defaults")]
    pub yes: bool,
}

#[derive(Args)]
pub struct CleanArgs {
    /// Clean mode: full, debug-only, incremental-only, deps-only.
    #[arg(short = 'm', long = "mode", value_enum, ignore_case = true)]
    pub mode: Option<CliCleanMode>,

    /// Skip confirmation prompts.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Preview what would be cleaned without changing files.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Only include projects at least this large for this run (in MB, overrides config).
    #[arg(
        long = "min-size-mb",
        visible_alias = "min-size",
        value_name = "MB",
        value_parser = clap::value_parser!(u64).range(0..=MAX_MIN_SIZE_MB)
    )]
    pub min_size: Option<u64>,

    /// Only clean projects not modified in N days (filter only).
    #[arg(
        long = "older-than",
        value_name = "DAYS",
        value_parser = clap::value_parser!(u32).range(0..=MAX_OLDER_THAN_DAYS)
    )]
    pub older_than: Option<u32>,

    /// Override the configured projects directory for this run.
    #[arg(long = "path", value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum CliCleanMode {
    Full,
    // NOTE: `PossibleValue` in clap 4.6 only supports hidden `alias`
    // (no `visible_alias`), so short forms stay functional but hidden.
    // `Arg` aliases below do use `visible_alias` where supported.
    #[value(name = "debug-only", alias = "debug")]
    DebugOnly,
    #[value(name = "incremental-only", alias = "incremental")]
    IncrementalOnly,
    #[value(name = "deps-only", alias = "deps")]
    DepsOnly,
}

impl From<CliCleanMode> for CleanMode {
    fn from(mode: CliCleanMode) -> Self {
        match mode {
            CliCleanMode::Full => CleanMode::Full,
            CliCleanMode::DebugOnly => CleanMode::DebugOnly,
            CliCleanMode::IncrementalOnly => CleanMode::IncrementalOnly,
            CliCleanMode::DepsOnly => CleanMode::DepsOnly,
        }
    }
}

#[derive(Args)]
pub struct ScanArgs {
    /// Override the configured projects directory for this run.
    #[arg(long = "path", value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    pub path: Option<String>,

    /// Only show projects at least this large for this run (in MB, overrides config).
    #[arg(
        long = "min-size-mb",
        visible_alias = "min-size",
        value_name = "MB",
        value_parser = clap::value_parser!(u64).range(0..=MAX_MIN_SIZE_MB)
    )]
    pub min_size: Option<u64>,

    /// Only show projects not modified in N days (display filter only).
    #[arg(
        long = "older-than",
        value_name = "DAYS",
        value_parser = clap::value_parser!(u32).range(0..=MAX_OLDER_THAN_DAYS)
    )]
    pub older_than: Option<u32>,
}

#[derive(Args)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub action: SettingsAction,
}

#[derive(Subcommand)]
pub enum SettingsAction {
    /// Display current configuration.
    Show,

    /// Update configuration values.
    Config(SettingsConfigArgs),

    /// Reset configuration to defaults.
    Reset(SettingsResetArgs),
}

#[derive(Args)]
pub struct SettingsConfigArgs {
    /// Set the projects directory path.
    #[arg(long = "projects-dir", value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    pub projects_dir: Option<String>,

    /// Set the scanning scope: tauri-only, tauri-and-rust, rust-only.
    #[arg(long = "scope", value_enum, ignore_case = true)]
    pub scope: Option<Scope>,

    /// Set the clean behavior: delete or trash.
    #[arg(long = "clean-behavior", value_enum, ignore_case = true)]
    pub clean_behavior: Option<CleanBehavior>,

    /// Set the default clean mode: full, debug-only, incremental-only, deps-only.
    #[arg(long = "default-mode", value_enum, ignore_case = true)]
    pub default_mode: Option<DefaultMode>,

    /// Set the minimum artifact size in MB to include (0 = show all).
    #[arg(
        long = "min-size-mb",
        value_name = "MB",
        value_parser = clap::value_parser!(u64).range(0..=MAX_MIN_SIZE_MB)
    )]
    pub min_size_mb: Option<u64>,

    /// Set the comma-separated list of project names to ignore (empty clears the list).
    #[arg(long = "ignored-projects", value_name = "NAMES")]
    pub ignored_projects: Option<String>,
}

#[derive(Args)]
pub struct SettingsResetArgs {
    /// Skip confirmation prompt.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

#[derive(Args)]
pub struct InspectArgs {
    /// Path to the project to inspect (explicit path; ignores stored filters).
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub project: PathBuf,

    /// Override the scope filter for inspection (default: tauri-and-rust).
    #[arg(long = "scope", value_enum, ignore_case = true)]
    pub scope: Option<Scope>,
}

/// Central application runner used by both `deoxidizer` and `deox` binaries.
pub fn run_app() {
    // Show the invoked binary name (deoxidizer vs deox) in help/usage output.
    // `name` above stays "deoxidizer" so `--version` output is identical.
    // NOTE: argv[0] is display-only here (usage line). A symlinked argv[0]
    // never changes dispatch, config paths, or update behavior; both
    // invokers share one parser and one library entry point.
    let bin_name = std::env::args()
        .next()
        .as_deref()
        .map(std::path::Path::new)
        .and_then(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "deoxidizer".to_string());
    let matches = Cli::command().bin_name(bin_name).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    if cli.update {
        crate::updater::run_update();
        return;
    }

    match cli.command {
        Some(Commands::Setup(args)) => {
            crate::setup::run_setup(args);
        }
        Some(Commands::Clean(args)) => {
            run_clean(args);
        }
        Some(Commands::Scan(args)) => {
            run_scan(args);
        }
        Some(Commands::Settings(args)) => {
            crate::settings::run_settings(args);
        }
        Some(Commands::Inspect(args)) => {
            run_inspect(args);
        }
        None => {
            let config = match Config::load_or_default() {
                Ok(config) => config,
                Err(error) => exit_with_config_error(error),
            };
            if !Config::config_path().exists() {
                eprintln!(
                    "No configuration found. Run 'deox setup' first, or 'deox setup --default' for quick defaults."
                );
                eprintln!();
                eprintln!("Running scan with defaults for now...\n");
            }
            let projects = match crate::scanner::scan(&config) {
                Ok(projects) => projects,
                Err(error) => {
                    eprintln!(
                        "Scan failed for '{}' (config {}): {error}.",
                        config.projects_dir,
                        Config::config_path().display()
                    );
                    std::process::exit(1);
                }
            };
            crate::display::print_scan_results(&projects);
        }
    }
}

fn run_clean(args: CleanArgs) {
    use crate::cleaner::{self, CleanMode};
    use crate::display;
    use crate::scanner;
    use colored::Colorize;
    use humansize::{format_size, BINARY};

    let mut config = match Config::load() {
        Ok(config) => config,
        Err(ConfigError::Missing(_)) => {
            eprintln!("No configuration found. Run 'deox setup' before cleaning.");
            std::process::exit(1);
        }
        Err(error) => exit_with_config_error(error),
    };

    apply_scan_overrides(&mut config, args.path.as_ref(), args.min_size);

    let mode: CleanMode = args
        .mode
        .map(Into::into)
        .unwrap_or_else(|| config.default_mode.into());

    println!("🔍 Scanning {}...", config.projects_dir);
    let mut projects = match scanner::scan(&config) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!(
                "Scan failed for '{}' (config {}): {error}.",
                config.projects_dir,
                Config::config_path().display()
            );
            std::process::exit(1);
        }
    };

    if let Some(days) = args.older_than {
        retain_older_than(&mut projects, days);
    }

    if projects.is_empty() {
        display::print_scan_results(&projects);
        return;
    }

    display::print_scan_results(&projects);

    let total_estimate: u64 = projects
        .iter()
        .map(|p| {
            cleaner::estimate_freed(p, &mode).unwrap_or_else(|error| {
                eprintln!(
                    "Cannot estimate {} ({}) for mode {mode}: {error}.",
                    p.name,
                    p.path.display()
                );
                std::process::exit(1);
            })
        })
        .fold(0u64, |acc, bytes| acc.saturating_add(bytes));

    println!("  Mode: {}\n  Behavior: {}\n", mode, config.clean_behavior);

    if args.dry_run {
        println!(
            "  {} Would free {} across {}.",
            "[DRY RUN]".yellow().bold(),
            format_size(total_estimate, BINARY).green().bold(),
            project_count(projects.len()),
        );
        return;
    }

    if !args.yes {
        use dialoguer::Confirm;
        // Declining (Ok(false)) cancels with exit 0; only I/O failure exits 1.
        match Confirm::new()
            .with_prompt(format!(
                "Clean {} to reclaim ~{}?",
                project_count(projects.len()),
                format_size(total_estimate, BINARY),
            ))
            .default(true)
            .interact()
        {
            Ok(true) => {}
            Ok(false) => {
                println!("Cancelled.");
                return;
            }
            Err(error) => {
                eprintln!("Confirmation failed: {error}.");
                std::process::exit(1);
            }
        }
    }

    let mut total_freed = 0u64;
    let mut cleaned = 0usize;
    let mut errors = 0usize;

    for project in &projects {
        let result = cleaner::clean_project(project, &mode, &config.clean_behavior, false);
        display::print_clean_result(&project.name, &result, &mode);
        match result {
            cleaner::CleanResult::Cleaned { bytes_freed } => {
                total_freed = total_freed.saturating_add(bytes_freed);
                cleaned += 1;
            }
            cleaner::CleanResult::Partial { bytes_freed, .. } => {
                total_freed = total_freed.saturating_add(bytes_freed);
                cleaned += 1;
                errors += 1;
            }
            cleaner::CleanResult::Error { .. } => errors += 1,
            _ => {}
        }
    }

    display::print_clean_summary(total_freed, cleaned, errors, &mode);
    if errors > 0 {
        eprintln!("Clean completed with errors.");
        std::process::exit(1);
    }
}

fn run_scan(args: ScanArgs) {
    use crate::config::Config;
    use crate::display;
    use crate::scanner;

    let mut config = match Config::load_or_default() {
        Ok(config) => config,
        Err(error) => exit_with_config_error(error),
    };

    apply_scan_overrides(&mut config, args.path.as_ref(), args.min_size);

    println!("🔍 Scanning {}...\n", config.projects_dir);
    let mut projects = match scanner::scan(&config) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!(
                "Scan failed for '{}' (config {}): {error}.",
                config.projects_dir,
                Config::config_path().display()
            );
            std::process::exit(1);
        }
    };

    if let Some(days) = args.older_than {
        retain_older_than(&mut projects, days);
    }

    display::print_scan_results(&projects);
}

fn run_inspect(args: InspectArgs) {
    use crate::config::Scope;
    use crate::display;
    use crate::scanner::scan_dir;

    let project_path = args.project.as_path();
    // Avoid TOCTOU (`exists()` then use): act on `canonicalize` directly and
    // map `NotFound` to the user-facing missing-path error.
    let requested_path = match project_path.canonicalize() {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Path does not exist: {}", args.project.display());
            std::process::exit(1);
        }
        Err(_) => project_path.to_path_buf(),
    };

    // Explicit-path inspection bypasses the configured scope, min-size, and
    // ignored-projects filters by design: the user named a concrete project,
    // so it is inspected regardless of scan preferences. The broad
    // TauriAndRust scope below only decides which project kinds are
    // recognizable here, and `--scope` can narrow it. Clap validates
    // `--scope` (exit 2) via `ValueEnum`; the tool writes kebab-case via
    // `Display` and the config file requires kebab-case strict serde;
    // `from_str_loose` in `config.rs` is only for CLI/settings input compat.
    let scope = args.scope.unwrap_or(Scope::TauriAndRust);

    let projects = match scan_dir(project_path, &scope) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!("Scan failed for '{}': {error}.", project_path.display());
            std::process::exit(1);
        }
    };
    let projects = if projects.is_empty() {
        let parent = project_path.parent().unwrap_or(project_path);
        match scan_dir(parent, &scope) {
            Ok(projects) => projects,
            Err(error) => {
                eprintln!("Scan failed for '{}': {error}.", parent.display());
                std::process::exit(1);
            }
        }
    } else {
        projects
    };

    if let Some(project) = find_inspection_project(projects, &requested_path) {
        display::print_inspection(&project);
    } else {
        eprintln!("No Rust/Tauri project found at: {}", args.project.display());
        std::process::exit(1);
    }
}

fn find_inspection_project(
    projects: Vec<crate::project::DiscoveredProject>,
    requested_path: &std::path::Path,
) -> Option<crate::project::DiscoveredProject> {
    // Canonicalize both sides for comparison; keep exact match plus
    // `starts_with` (inspecting a file inside a project). The old substring
    // `contains` fallback is intentionally dropped: it could match unrelated
    // siblings sharing a name fragment.
    let requested = canonicalize_or_self(requested_path);
    let mut projects = projects.into_iter();
    projects
        .find(|project| canonicalize_or_self(&project.path) == requested)
        .or_else(|| {
            projects.find(|project| {
                let path = canonicalize_or_self(&project.path);
                requested.starts_with(&path)
            })
        })
}

/// Canonicalize a path for comparison, falling back to the original path
/// when resolution fails (e.g. removed or unreadable entries).
fn canonicalize_or_self(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Keep only projects not modified in the last `days` days.
fn retain_older_than(projects: &mut Vec<crate::project::DiscoveredProject>, days: u32) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86400));
    let Some(cutoff) = cutoff else {
        eprintln!("Invalid --older-than value '{days}': out of range.");
        std::process::exit(2);
    };
    projects.retain(|p| p.last_modified.is_some_and(|t| t < cutoff));
}

/// Apply `--path` / `--min-size-mb` overrides, then re-validate.
///
/// CLI-caused validation failures exit 2 (usage error), not 1: the stored
/// config already loaded, so emptiness/overflow must come from the flags.
fn apply_scan_overrides(config: &mut Config, path: Option<&String>, min_size: Option<u64>) {
    if let Some(path) = path {
        if path.trim().is_empty() {
            eprintln!("Invalid --path: must not be empty.");
            std::process::exit(2);
        }
        config.projects_dir = path.clone();
    }
    if let Some(min_mb) = min_size {
        config.min_size_mb = min_mb;
    }
    if let Err(error) = config.validate() {
        eprintln!("Invalid configuration: {error}.");
        std::process::exit(2);
    }
}

/// Format `1 project` vs `N projects` for user-facing counts.
fn project_count(count: usize) -> String {
    if count == 1 {
        "1 project".to_string()
    } else {
        format!("{count} projects")
    }
}

fn exit_with_config_error(error: ConfigError) -> ! {
    eprintln!("Configuration error: {error}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_debug_assert() {
        Cli::command().debug_assert();
    }
}
