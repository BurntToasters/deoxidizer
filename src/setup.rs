use crate::config::{CleanBehavior, Config, DefaultMode, Scope, CONFIG_VERSION};
use colored::Colorize;
use dialoguer::{Input, Select};

/// Run the setup wizard.
/// If `use_defaults` is true, apply defaults without prompting.
pub fn run_setup(use_defaults: bool) {
    println!();
    println!("  {} deoxidizer setup", "🔧".bold());
    println!("  {}", "─".repeat(40).dimmed());
    println!();

    if use_defaults {
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
                eprintln!("  {} Failed to save config: {}", "✗".red().bold(), e);
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
        .unwrap_or_else(|_| {
            eprintln!("Setup cancelled.");
            std::process::exit(1);
        });

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
            eprintln!("Setup cancelled: {error}");
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
            eprintln!("Setup cancelled: {error}");
            std::process::exit(1);
        });

    let clean_behavior = match behavior_idx {
        0 => CleanBehavior::Delete,
        1 => CleanBehavior::Trash,
        _ => CleanBehavior::Delete,
    };

    let config = Config {
        version: CONFIG_VERSION,
        projects_dir,
        scope,
        clean_behavior,
        default_mode: DefaultMode::Full,
        min_size_mb: 0,
        ignored_projects: Vec::new(),
    };

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
            eprintln!("  {} Failed to save config: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
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
    println!();
    println!(
        "  Run {} to scan for artifacts, or {} to clean.",
        "deox scan".green(),
        "deox clean".green()
    );
    println!();
}
