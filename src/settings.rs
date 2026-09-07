use crate::cli::{SettingsAction, SettingsArgs, SettingsConfigArgs, SettingsResetArgs};
use crate::config::{CleanBehavior, Config, ConfigError, DefaultMode, Scope};
use colored::Colorize;

pub fn run_settings(args: SettingsArgs) {
    match args.action {
        SettingsAction::Show => show_settings(),
        SettingsAction::Config(config_args) => update_settings(config_args),
        SettingsAction::Reset(reset_args) => reset_settings(reset_args),
    }
}

fn show_settings() {
    let config_path = Config::config_path();

    let config = match Config::load() {
        Ok(config) => config,
        Err(ConfigError::Missing(_)) => {
            println!();
            println!(
                "  {} No configuration file found at {}",
                "ℹ".blue(),
                config_path.display()
            );
            println!("  Run {} to create one.", "deox setup".green());
            println!();
            return;
        }
        Err(error) => {
            eprintln!("  {} Failed to load settings: {}", "✗".red().bold(), error);
            std::process::exit(1);
        }
    };

    println!();
    println!("  {} deoxidizer settings", "⚙".bold());
    println!("  {}", "─".repeat(40).dimmed());
    println!();
    println!("  {:<20} {}", "Config file:".bold(), config_path.display());
    println!("  {:<20} {}", "Config version:".bold(), config.version);
    println!(
        "  {:<20} {}",
        "Projects dir:".bold(),
        config.projects_dir.cyan()
    );
    println!(
        "  {:<20} {}",
        "Scope:".bold(),
        config.scope.to_string().cyan()
    );
    println!(
        "  {:<20} {}",
        "Clean behavior:".bold(),
        config.clean_behavior.to_string().cyan()
    );
    println!(
        "  {:<20} {}",
        "Default mode:".bold(),
        config.default_mode.to_string().cyan()
    );
    println!(
        "  {:<20} {} MB",
        "Min size filter:".bold(),
        config.min_size_mb
    );

    if config.ignored_projects.is_empty() {
        println!("  {:<20} {}", "Ignored projects:".bold(), "(none)".dimmed());
    } else {
        println!(
            "  {:<20} {}",
            "Ignored projects:".bold(),
            config.ignored_projects.join(", ")
        );
    }
    println!();
}

fn update_settings(args: SettingsConfigArgs) {
    let mut config = match Config::load_or_default() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("  {} Failed to load settings: {}", "✗".red().bold(), error);
            std::process::exit(1);
        }
    };
    let mut changed = false;
    let mut invalid = false;

    if let Some(ref dir) = args.projects_dir {
        config.projects_dir = dir.clone();
        changed = true;
        println!("  {} projects-dir = {}", "✓".green(), dir.cyan());
    }

    if let Some(ref scope_str) = args.scope {
        match Scope::from_str_loose(scope_str) {
            Some(scope) => {
                config.scope = scope;
                changed = true;
                println!("  {} scope = {}", "✓".green(), scope_str.cyan());
            }
            None => {
                invalid = true;
                eprintln!(
                    "  {} Invalid scope '{}'. Use: tauri-only, tauri-and-rust, rust-only",
                    "✗".red(),
                    scope_str
                );
            }
        }
    }

    if let Some(ref behavior_str) = args.clean_behavior {
        match CleanBehavior::from_str_loose(behavior_str) {
            Some(behavior) => {
                config.clean_behavior = behavior;
                changed = true;
                println!("  {} clean-behavior = {}", "✓".green(), behavior_str.cyan());
            }
            None => {
                invalid = true;
                eprintln!(
                    "  {} Invalid clean behavior '{}'. Use: delete, trash",
                    "✗".red(),
                    behavior_str
                );
            }
        }
    }

    if let Some(ref mode_str) = args.default_mode {
        match DefaultMode::from_str_loose(mode_str) {
            Some(mode) => {
                config.default_mode = mode;
                changed = true;
                println!("  {} default-mode = {}", "✓".green(), mode_str.cyan());
            }
            None => {
                invalid = true;
                eprintln!(
                    "  {} Invalid mode '{}'. Use: full, debug-only, incremental-only, deps-only",
                    "✗".red(),
                    mode_str
                );
            }
        }
    }

    if changed && !invalid {
        match config.save() {
            Ok(()) => println!("  {} Settings saved.", "✓".green().bold()),
            Err(e) => {
                eprintln!("  {} Failed to save: {}", "✗".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
    if invalid {
        std::process::exit(2);
    }
    if !changed
        && args.projects_dir.is_none()
        && args.scope.is_none()
        && args.clean_behavior.is_none()
        && args.default_mode.is_none()
    {
        println!("No settings specified. Use --projects-dir, --scope, --clean-behavior, or --default-mode.");
        println!("Run 'deox settings show' to view current settings.");
    }
}

fn reset_settings(args: SettingsResetArgs) {
    if !args.yes {
        use dialoguer::Confirm;
        let confirmed = Confirm::new()
            .with_prompt("Reset all settings to defaults?")
            .default(false)
            .interact()
            .unwrap_or(false);
        if !confirmed {
            println!("Cancelled.");
            return;
        }
    }

    let config = Config::default();
    match config.save() {
        Ok(()) => {
            println!("  {} Settings reset to defaults.", "✓".green().bold());
        }
        Err(e) => {
            eprintln!("  {} Failed to reset: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
}
