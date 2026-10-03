use crate::cleaner::CleanMode;
use crate::config::{CleanBehavior, Config, ConfigError, DefaultMode, Scope};
use crate::project::DiscoveredProject;
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

    /// Check for updates and self-update the binary (cannot be combined
    /// with a subcommand).
    #[arg(short = 'u', long = "update")]
    pub update: bool,

    /// Check whether a newer release exists without installing it.
    #[arg(long = "check-update", conflicts_with = "update")]
    pub check_update: bool,

    /// Use a specific config file instead of ~/.deox_config.
    /// (Portable installs, testing, multiple profiles. `inspect` ignores
    /// stored config by design, so this flag is a no-op there.)
    #[arg(
        long = "config",
        global = true,
        value_name = "PATH",
        value_hint = clap::ValueHint::FilePath
    )]
    pub config: Option<PathBuf>,
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

    /// Interactively choose which projects to clean (needs a terminal).
    #[arg(short = 's', long = "select")]
    pub select: bool,

    /// In full mode, keep Tauri installer bundles (target/*/release/bundle).
    #[arg(long = "keep-bundles")]
    pub keep_bundles: bool,

    /// Print results as JSON on stdout (no table).
    #[arg(long = "json")]
    pub json: bool,
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

    /// Print results as JSON on stdout (no table).
    #[arg(long = "json")]
    pub json: bool,
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
    /// Path to the project, or any file or folder inside it (ignores stored
    /// filters).
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    pub project: PathBuf,

    /// Override the scope filter for inspection (default: tauri-and-rust).
    #[arg(long = "scope", value_enum, ignore_case = true)]
    pub scope: Option<Scope>,
}

/// Central application runner used by both `deoxidizer` and `deox` binaries.
pub fn run_app() {
    #[cfg(windows)]
    {
        // Enable ANSI escape processing on legacy Windows consoles.
        let _ = colored::control::set_virtual_terminal(true);
    }
    // Show the invoked binary name (deoxidizer vs deox) in help/usage output.
    // `name` above stays "deoxidizer" so `--version` output is identical.
    // argv[0] is display-only: it never changes dispatch, config paths, or
    // update behavior.
    let bin_name = std::env::args()
        .next()
        .as_deref()
        .map(std::path::Path::new)
        .and_then(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "deoxidizer".to_string());
    let mut command = Cli::command().bin_name(bin_name);
    let matches = command.clone().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    if (cli.update || cli.check_update) && cli.command.is_some() {
        command
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "--update/--check-update cannot be combined with a subcommand",
            )
            .exit();
    }
    if cli.update || cli.check_update {
        crate::updater::run_update(cli.check_update);
        return;
    }

    let config_override = cli.config.clone();
    let config_path = config_override.as_ref();

    match cli.command {
        Some(Commands::Setup(args)) => crate::setup::run_setup(args, config_path),
        Some(Commands::Clean(args)) => run_clean(args, config_path),
        Some(Commands::Scan(args)) => run_scan(args, config_path),
        Some(Commands::Settings(args)) => crate::settings::run_settings(args, config_path),
        Some(Commands::Inspect(args)) => run_inspect(args),
        None => {
            let config = load_config_or_default(config_path);
            if !effective_config_path(config_path).exists() {
                eprintln!(
                    "No configuration found. Run 'deox setup' first, or 'deox setup --default' for quick defaults."
                );
                eprintln!();
                eprintln!("Running scan with defaults for now...\n");
            }
            let projects = scan_or_exit(&config, config_path);
            crate::display::print_scan_results(&projects);
        }
    }
}

fn scan_or_exit(config: &Config, config_override: Option<&PathBuf>) -> Vec<DiscoveredProject> {
    match crate::scanner::scan(config) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!(
                "Scan failed for '{}' (config {}): {error}.",
                config.projects_dir,
                effective_config_path(config_override).display()
            );
            std::process::exit(1);
        }
    }
}

/// Config path shown in messages: explicit `--config` or the default home path.
fn effective_config_path(config_override: Option<&PathBuf>) -> PathBuf {
    config_override.cloned().unwrap_or_else(Config::config_path)
}

/// Strict load for commands that refuse to run without a config file.
/// An explicit `--config` path is honored the same way as the default path:
/// a missing file refuses, corrupt content fails closed.
fn load_config_strict(config_override: Option<&PathBuf>) -> Config {
    let result = match config_override {
        Some(path) => Config::load_from(path),
        None => Config::load(),
    };
    match result {
        Ok(config) => config,
        Err(ConfigError::Missing(_)) => {
            eprintln!("No configuration found. Run 'deox setup' before cleaning.");
            std::process::exit(1);
        }
        Err(error) => exit_with_config_error(error),
    }
}

/// Lenient load for commands that fall back to defaults when no config exists.
fn load_config_or_default(config_override: Option<&PathBuf>) -> Config {
    let result = match config_override {
        Some(path) => Config::load_or_default_from(path),
        None => Config::load_or_default(),
    };
    match result {
        Ok(config) => config,
        Err(error) => exit_with_config_error(error),
    }
}

fn run_clean(args: CleanArgs, config_override: Option<&PathBuf>) {
    use crate::cleaner::{self, CleanMode, CleanOptions, CleanResult};
    use crate::display;
    use colored::Colorize;
    use humansize::{format_size, BINARY};
    use std::io::IsTerminal;

    let mut config = load_config_strict(config_override);
    apply_scan_overrides(&mut config, args.path.as_ref(), args.min_size);
    let mode: CleanMode = args
        .mode
        .map(Into::into)
        .unwrap_or_else(|| config.default_mode.into());
    let options = CleanOptions {
        dry_run: args.dry_run,
        keep_bundles: args.keep_bundles,
    };
    let human = !args.json;
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if args.select && !interactive {
        eprintln!("--select needs an interactive terminal; nothing was cleaned.");
        std::process::exit(1);
    }

    if human {
        println!("🔍 Scanning {}...", config.projects_dir);
    }
    let report = match crate::scanner::scan_with_report(&config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "Scan failed for '{}' (config {}): {error}.",
                config.projects_dir,
                effective_config_path(config_override).display()
            );
            std::process::exit(1);
        }
    };
    let mut projects = report.projects;
    if let Some(days) = args.older_than {
        retain_older_than(&mut projects, days);
    }
    if human {
        display::print_scan_results(&projects);
    }

    // Measure every project live (the same plan the clean executes). A
    // project that cannot be analyzed or measured is reported and excluded;
    // the rest still proceed, and the run exits 1.
    let mut errors = report.analysis_failures;
    let mut failures: Vec<serde_json::Value> = Vec::new();
    let mut candidates: Vec<(DiscoveredProject, u64)> = Vec::new();
    for project in projects {
        match cleaner::estimate_with(&project, &mode, &options) {
            Ok(bytes) => candidates.push((project, bytes)),
            Err(error) => {
                errors += 1;
                eprintln!(
                    "Cannot measure {} ({}): {error}. Skipping it.",
                    project.name,
                    display::display_path(&project.path)
                );
                failures.push(serde_json::json!({
                    "name": project.name,
                    "path": project.path,
                    "result": "error",
                    "bytes": 0,
                    "message": error,
                }));
            }
        }
    }

    if args.select && !candidates.is_empty() {
        let items: Vec<String> = candidates
            .iter()
            .map(|(p, bytes)| {
                format!(
                    "{} — {} ({})",
                    p.name,
                    display::display_path(&p.path),
                    format_size(*bytes, BINARY)
                )
            })
            .collect();
        let defaults = vec![true; items.len()];
        let chosen = match dialoguer::MultiSelect::new()
            .with_prompt("Select projects to clean (space toggles, enter confirms)")
            .items(&items)
            .defaults(&defaults)
            .interact()
        {
            Ok(chosen) => chosen,
            Err(error) => {
                eprintln!("Selection failed: {error}.");
                std::process::exit(1);
            }
        };
        candidates = candidates
            .into_iter()
            .enumerate()
            .filter(|(index, _)| chosen.contains(index))
            .map(|(_, candidate)| candidate)
            .collect();
    }

    let total_estimate = candidates
        .iter()
        .fold(0u64, |acc, (_, bytes)| acc.saturating_add(*bytes));

    if human && mode == CleanMode::Full && !args.keep_bundles {
        let with_bundles = candidates
            .iter()
            .filter(|(p, _)| !cleaner::bundle_dirs(&p.artifact_dir).is_empty())
            .count();
        if with_bundles > 0 {
            let subject = if with_bundles == 1 {
                "1 project contains".to_string()
            } else {
                format!("{with_bundles} projects contain")
            };
            println!(
                "  {} {subject} Tauri installer bundles (target/*/release/bundle) that full mode deletes. Use --keep-bundles to preserve them.\n",
                "⚠".yellow().bold(),
            );
        }
    }

    if human {
        println!("  Mode: {}\n  Behavior: {}\n", mode, config.clean_behavior);
    }

    if args.dry_run {
        if human {
            println!(
                "  {} Would free {} across {}.",
                "[DRY RUN]".yellow().bold(),
                format_size(total_estimate, BINARY).green().bold(),
                project_count(candidates.len()),
            );
        } else {
            let mut entries: Vec<serde_json::Value> = candidates
                .iter()
                .map(|(p, bytes)| {
                    serde_json::json!({
                        "name": p.name,
                        "path": p.path,
                        "result": "would-clean",
                        "bytes": bytes,
                    })
                })
                .collect();
            entries.extend(failures);
            print_clean_json(&mode, &config.clean_behavior, true, total_estimate, entries);
        }
        exit_if_errors(errors, human);
        return;
    }

    if candidates.is_empty() {
        if !human {
            print_clean_json(&mode, &config.clean_behavior, false, 0, failures);
        }
        exit_if_errors(errors, human);
        return;
    }

    if !args.yes {
        // Declining (Ok(false)) cancels with exit 0; only I/O failure exits 1.
        match dialoguer::Confirm::new()
            .with_prompt(format!(
                "Clean {} to reclaim ~{}?",
                project_count(candidates.len()),
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
    let mut entries = failures;
    for (project, _) in &candidates {
        let result = cleaner::clean_project_with(project, &mode, &config.clean_behavior, &options);
        if human {
            display::print_clean_result(project, &result);
        }
        let (label, bytes, message) = match &result {
            CleanResult::Cleaned { bytes_freed } => {
                total_freed = total_freed.saturating_add(*bytes_freed);
                cleaned += 1;
                ("cleaned", *bytes_freed, None)
            }
            CleanResult::Partial {
                bytes_freed,
                message,
            } => {
                total_freed = total_freed.saturating_add(*bytes_freed);
                cleaned += 1;
                errors += 1;
                ("partial", *bytes_freed, Some(message.clone()))
            }
            CleanResult::Skipped { reason } => ("skipped", 0, Some(reason.clone())),
            CleanResult::Error { message } => {
                errors += 1;
                ("error", 0, Some(message.clone()))
            }
        };
        entries.push(serde_json::json!({
            "name": project.name,
            "path": project.path,
            "result": label,
            "bytes": bytes,
            "message": message,
        }));
    }

    if human {
        display::print_clean_summary(
            total_freed,
            cleaned,
            errors,
            &mode,
            config.clean_behavior == CleanBehavior::Trash,
        );
    } else {
        print_clean_json(&mode, &config.clean_behavior, false, total_freed, entries);
    }
    exit_if_errors(errors, human);
}

fn print_clean_json(
    mode: &crate::cleaner::CleanMode,
    behavior: &CleanBehavior,
    dry_run: bool,
    bytes: u64,
    projects: Vec<serde_json::Value>,
) {
    let value = serde_json::json!({
        "mode": mode.to_string(),
        "behavior": behavior.to_string(),
        "dry_run": dry_run,
        "bytes": bytes,
        "projects": projects,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

fn exit_if_errors(errors: usize, human: bool) {
    if errors > 0 {
        if human {
            eprintln!("Clean completed with errors.");
        }
        std::process::exit(1);
    }
}

fn run_scan(args: ScanArgs, config_override: Option<&PathBuf>) {
    let mut config = load_config_or_default(config_override);
    apply_scan_overrides(&mut config, args.path.as_ref(), args.min_size);
    if !args.json {
        println!("🔍 Scanning {}...\n", config.projects_dir);
    }
    let mut projects = scan_or_exit(&config, config_override);
    if let Some(days) = args.older_than {
        retain_older_than(&mut projects, days);
    }
    if args.json {
        crate::display::print_scan_json(&projects);
    } else {
        crate::display::print_scan_results(&projects);
    }
}

fn run_inspect(args: InspectArgs) {
    use crate::scanner::{enclosing_project_root, scan_dir};

    let requested_path = match dunce::canonicalize(&args.project) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Path does not exist: {}", args.project.display());
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("Cannot resolve {}: {error}", args.project.display());
            std::process::exit(1);
        }
    };

    // Explicit-path inspection bypasses the configured scope, min-size, and
    // ignored-projects filters by design. Only the enclosing project (or
    // workspace) is scanned — never its parent folder.
    let scope = args.scope.unwrap_or(Scope::TauriAndRust);
    let Some(base) = enclosing_project_root(&requested_path) else {
        eprintln!("No Rust/Tauri project found at: {}", args.project.display());
        std::process::exit(1);
    };
    let projects = match scan_dir(&base, &scope) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!("Scan failed for '{}': {error}.", base.display());
            std::process::exit(1);
        }
    };

    if let Some(project) = find_inspection_project(projects, &requested_path) {
        crate::display::print_inspection(&project);
    } else {
        eprintln!(
            "No Rust/Tauri project with build artifacts found at: {}",
            args.project.display()
        );
        std::process::exit(1);
    }
}

fn find_inspection_project(
    projects: Vec<DiscoveredProject>,
    requested_path: &std::path::Path,
) -> Option<DiscoveredProject> {
    // Most specific project root containing the requested path wins.
    projects
        .into_iter()
        .filter_map(|project| {
            let path = dunce::canonicalize(&project.path).unwrap_or_else(|_| project.path.clone());
            requested_path
                .starts_with(&path)
                .then(|| (path.components().count(), project))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, project)| project)
}

/// Keep only projects not modified in the last `days` days.
fn retain_older_than(projects: &mut Vec<DiscoveredProject>, days: u32) {
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
        config.projects_dir = crate::config::normalize_projects_dir(path);
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
