use crate::cleaner::CleanMode;
use crate::config::{Config, ConfigError};
use clap::{Args, Parser, Subcommand, ValueEnum};

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
    #[arg(short = 'u', long = "update", global = true)]
    pub update: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the interactive setup wizard.
    Setup(SetupArgs),

    /// Scan and clean build artifacts.
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
}

#[derive(Args)]
pub struct CleanArgs {
    /// Clean mode: full, debug-only, incremental-only, deps-only.
    #[arg(short = 'm', long = "mode", value_enum)]
    pub mode: Option<CliCleanMode>,

    /// Skip confirmation prompts.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Preview what would be cleaned without actually deleting.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Only clean projects not modified in N days.
    #[arg(long = "older-than")]
    pub older_than: Option<u32>,

    /// Override the configured projects directory for this run.
    #[arg(long = "path")]
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CliCleanMode {
    Full,
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
    #[arg(long = "path")]
    pub path: Option<String>,

    /// Only show projects larger than this (e.g. 500, in MB).
    #[arg(long = "min-size")]
    pub min_size: Option<u64>,
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
    #[arg(long = "projects-dir")]
    pub projects_dir: Option<String>,

    /// Set the scanning scope: tauri-only, tauri-and-rust, rust-only.
    #[arg(long = "scope")]
    pub scope: Option<String>,

    /// Set the clean behavior: delete or trash.
    #[arg(long = "clean-behavior")]
    pub clean_behavior: Option<String>,

    /// Set the default clean mode: full, debug-only, incremental-only, deps-only.
    #[arg(long = "default-mode")]
    pub default_mode: Option<String>,
}

#[derive(Args)]
pub struct SettingsResetArgs {
    /// Skip confirmation prompt.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

#[derive(Args)]
pub struct InspectArgs {
    /// Path to the project to inspect.
    pub project: String,
}

/// Central application runner used by both `deoxidizer` and `deox` binaries.
pub fn run_app() {
    let cli = Cli::parse();

    if cli.update {
        crate::updater::run_update();
        return;
    }

    match cli.command {
        Some(Commands::Setup(args)) => {
            crate::setup::run_setup(args.use_defaults);
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
            let projects = crate::scanner::scan(&config);
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

    if let Some(ref path) = args.path {
        config.projects_dir = path.clone();
    }

    let mode: CleanMode = args
        .mode
        .map(Into::into)
        .unwrap_or_else(|| config.default_mode.into());

    println!("🔍 Scanning {}...", config.projects_dir);
    let mut projects = scanner::scan(&config);

    if let Some(days) = args.older_than {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86400));
        let Some(cutoff) = cutoff else {
            eprintln!("Invalid --older-than value.");
            std::process::exit(2);
        };
        projects.retain(|p| p.last_modified.is_some_and(|t| t < cutoff));
    }

    if projects.is_empty() {
        println!("No projects with cleanable artifacts found.");
        return;
    }

    display::print_scan_results(&projects);

    let total_estimate: u64 = projects
        .iter()
        .map(|p| {
            cleaner::estimate_freed(p, &mode).unwrap_or_else(|error| {
                eprintln!("Cannot estimate {}: {}", p.name, error);
                std::process::exit(1);
            })
        })
        .sum();

    println!("  Mode: {}\n  Behavior: {}\n", mode, config.clean_behavior);

    if args.dry_run {
        println!(
            "  {} Would free {} across {} project(s).",
            "[DRY RUN]".yellow().bold(),
            format_size(total_estimate, BINARY).green().bold(),
            projects.len(),
        );
        return;
    }

    if !args.yes {
        use dialoguer::Confirm;
        let confirmed = Confirm::new()
            .with_prompt(format!(
                "Clean {} project(s) to reclaim ~{}?",
                projects.len(),
                format_size(total_estimate, BINARY),
            ))
            .default(true)
            .interact()
            .unwrap_or(false);

        if !confirmed {
            println!("Cancelled.");
            return;
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
                total_freed += bytes_freed;
                cleaned += 1;
            }
            cleaner::CleanResult::Partial { bytes_freed, .. } => {
                total_freed += bytes_freed;
                cleaned += 1;
                errors += 1;
            }
            cleaner::CleanResult::Error { .. } => errors += 1,
            _ => {}
        }
    }

    display::print_clean_summary(total_freed, cleaned, errors);
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

    if let Some(ref path) = args.path {
        config.projects_dir = path.clone();
    }

    if let Some(min_mb) = args.min_size {
        config.min_size_mb = min_mb;
    }

    println!("🔍 Scanning {}...\n", config.projects_dir);
    let projects = scanner::scan(&config);

    display::print_scan_results(&projects);
}

fn run_inspect(args: InspectArgs) {
    use crate::config::Scope;
    use crate::display;
    use crate::scanner::scan_dir;
    use std::path::Path;

    let project_path = Path::new(&args.project);
    if !project_path.exists() {
        eprintln!("Path does not exist: {}", args.project);
        std::process::exit(1);
    }
    let requested_path = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());

    let projects = scan_dir(project_path, &Scope::TauriAndRust);
    if projects.is_empty() {
        let parent = project_path.parent().unwrap_or(project_path);
        let projects = scan_dir(parent, &Scope::TauriAndRust);
        if let Some(p) = projects.into_iter().find(|p| {
            p.path == requested_path
                || p.artifact_dir.starts_with(&requested_path)
                || requested_path.starts_with(&p.path)
        }) {
            display::print_inspection(&p);
        } else {
            eprintln!("No Rust/Tauri project found at: {}", args.project);
            std::process::exit(1);
        }
    } else if let Some(p) = projects.into_iter().next() {
        display::print_inspection(&p);
    }
}

fn exit_with_config_error(error: ConfigError) -> ! {
    eprintln!("Configuration error: {error}");
    std::process::exit(1);
}
