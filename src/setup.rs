use crate::cli::SetupArgs;
use crate::config::{CleanBehavior, Config, DefaultMode, Scope, CONFIG_VERSION};
use colored::Colorize;
use dialoguer::{Confirm, Input, Select};

/// Run the setup wizard.
/// If `use_defaults` is true, apply defaults without prompting.
pub fn run_setup(args: SetupArgs) {
    let use_defaults = args.use_defaults;
    println!();
    println!("  {} deoxidizer setup", "🔧".bold());
    println!("  {}", "─".repeat(40).dimmed());
    println!();

    if use_defaults {
        let config_path = Config::config_path();
        if config_path.exists() && !args.yes {
            match Confirm::new()
                .with_prompt(format!(
                    "Existing config at {} will be overwritten. Continue?",
                    config_path.display()
                ))
                .default(false)
                .interact()
            {
                Ok(true) => {}
                Ok(false) => {
                    println!("Cancelled.");
                    return;
                }
                Err(error) => {
                    eprintln!("Setup cancelled: {error}.");
                    std::process::exit(1);
                }
            }
        }
        if config_path.exists() {
            backup_existing_config(&config_path);
        }
        let config = Config::default();
        match config.save() {
            Ok(()) => {
                println!(
                    "  {} Default configuration saved to {}",
                    "✓".green().bold(),
                    Config::config_path().display()
                );
                println!();
                print_config_summary(&config);
            }
            Err(e) => {
                eprintln!("  {} Failed to save config: {e}.", "✗".red().bold());
                std::process::exit(1);
            }
        }
        return;
    }

    // Step 1: Projects directory
    let default_dir = Config::default().projects_dir;
    let projects_dir: String = Input::new()
        .with_prompt("  Main projects/GitHub directory")
        .default(default_dir)
        .interact_text()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });
    if projects_dir.trim().is_empty() {
        eprintln!(
            "  {} Invalid projects directory: must not be empty.",
            "✗".red().bold()
        );
        std::process::exit(1);
    }

    // Validate directory exists
    let path = std::path::Path::new(&projects_dir);
    if !path.is_dir() {
        eprintln!(
            "  {} Directory does not exist: {}",
            "⚠".yellow(),
            projects_dir
        );
        eprintln!("  Creating it is up to you — config will be saved as-is.");
    }

    // Step 2: Project scope
    let scope_options = &[
        "tauri-only (default) — Only scan Tauri app projects",
        "tauri-and-rust — Scan both Tauri and plain Rust projects",
        "rust-only — Only scan plain Rust projects (no Tauri)",
    ];
    let scope_idx = Select::new()
        .with_prompt("  Project scope")
        .items(scope_options)
        .default(0)
        .interact()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });

    let scope = match scope_idx {
        0 => Scope::TauriOnly,
        1 => Scope::TauriAndRust,
        2 => Scope::RustOnly,
        _ => Scope::TauriOnly,
    };

    // Step 3: Clean behavior
    let behavior_options = &[
        "delete permanently (default) — Remove files immediately",
        "move to OS trash — Files can be recovered from Trash/Recycle Bin",
    ];
    let behavior_idx = Select::new()
        .with_prompt("  Default clean behavior")
        .items(behavior_options)
        .default(0)
        .interact()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });

    let clean_behavior = match behavior_idx {
        0 => CleanBehavior::Delete,
        1 => CleanBehavior::Trash,
        _ => CleanBehavior::Delete,
    };

    // Step 4: Default clean mode
    let mode_options = &[
        "full (default) — Remove the entire target/ directory",
        "debug-only — Remove debug/ dirs, keep release/",
        "incremental-only — Remove incremental caches only",
        "deps-only — Remove dependency compilation units only",
    ];
    let mode_idx = Select::new()
        .with_prompt("  Default clean mode")
        .items(mode_options)
        .default(0)
        .interact()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });

    let default_mode = match mode_idx {
        0 => DefaultMode::Full,
        1 => DefaultMode::DebugOnly,
        2 => DefaultMode::IncrementalOnly,
        3 => DefaultMode::DepsOnly,
        _ => DefaultMode::Full,
    };

    // Step 5: Minimum artifact size filter
    let min_size_mb: u64 = Input::new()
        .with_prompt("  Minimum artifact size to show (MB, 0 = show all)")
        .default(0)
        .interact_text()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });
    if min_size_mb.checked_mul(1024 * 1024).is_none() {
        eprintln!(
            "  {} Invalid minimum size {min_size_mb}: too large.",
            "✗".red().bold()
        );
        std::process::exit(1);
    }

    // Step 6: Ignored projects
    let ignored_input: String = Input::new()
        .with_prompt("  Ignored projects (comma-separated, blank = none)")
        .default(String::new())
        .allow_empty(true)
        .interact_text()
        .unwrap_or_else(|error| {
            eprintln!("Setup cancelled: {error}.");
            std::process::exit(1);
        });
    let ignored_projects: Vec<String> = ignored_input
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    let config = Config {
        version: CONFIG_VERSION,
        projects_dir,
        scope,
        clean_behavior,
        default_mode,
        min_size_mb,
        ignored_projects,
    };

    let config_path = Config::config_path();
    if config_path.exists() {
        backup_existing_config(&config_path);
    }
    match config.save() {
        Ok(()) => {
            println!();
            println!(
                "  {} Configuration saved to {}",
                "✓".green().bold(),
                Config::config_path().display()
            );
            println!();
            print_config_summary(&config);
        }
        Err(e) => {
            eprintln!("  {} Failed to save config: {e}.", "✗".red().bold());
            std::process::exit(1);
        }
    }
}

/// Best-effort backup of an existing config to `<path>.bak` before overwrite.
fn backup_existing_config(path: &std::path::Path) {
    let mut backup_name = path.as_os_str().to_owned();
    backup_name.push(".bak");
    let backup = std::path::PathBuf::from(backup_name);
    if let Err(error) = std::fs::copy(path, &backup) {
        eprintln!(
            "  {} Could not back up existing config to {}: {error}. Continuing.",
            "⚠".yellow(),
            backup.display()
        );
    }
}

fn print_config_summary(config: &Config) {
    println!("  {}", "Current settings:".bold());
    println!("    Projects dir:    {}", config.projects_dir.cyan());
    println!("    Scope:           {}", config.scope.to_string().cyan());
    println!(
        "    Clean behavior:  {}",
        config.clean_behavior.to_string().cyan()
    );
    println!(
        "    Default mode:    {}",
        config.default_mode.to_string().cyan()
    );
    println!(
        "    Min size:        {} MB",
        config.min_size_mb.to_string().cyan()
    );
    if config.ignored_projects.is_empty() {
        println!("    Ignored:         {}", "(none)".dimmed());
    } else {
        println!(
            "    Ignored:         {}",
            config.ignored_projects.join(", ").cyan()
        );
    }
    println!();
    println!(
        "  Run {} to scan for artifacts, or {} to clean.",
        "deox scan".green(),
        "deox clean".green()
    );
    println!();
}
