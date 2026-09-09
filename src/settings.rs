use crate::cli::{SettingsAction, SettingsArgs, SettingsConfigArgs, SettingsResetArgs};
use crate::config::{Config, ConfigError};
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
            eprintln!("  {} Failed to load settings: {error}.", "✗".red().bold());
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
            eprintln!("  {} Failed to load settings: {error}.", "✗".red().bold());
            std::process::exit(1);
        }
    };
    // Validate everything first; buffer success lines and only print after
    // the config saves, so a partial-valid invocation never prints success
    // for values that were not persisted.
    let mut changed = false;
    let mut invalid = false;
    let mut pending: Vec<String> = Vec::new();

    if let Some(dir) = args.projects_dir {
        if dir.trim().is_empty() {
            invalid = true;
            eprintln!("  {} Invalid projects-dir: must not be empty.", "✗".red());
        } else {
            config.projects_dir = dir.clone();
            changed = true;
            pending.push(format!("  {} projects-dir = {}", "✓".green(), dir.cyan()));
        }
    }

    // Clap `ValueEnum` already rejects unknown scope/behavior/mode values
    // (exit 2 with possible values); `from_str_loose` in `config.rs` remains
    // for CLI/settings input compat only; the tool writes kebab-case via
    // `Display` and the config file requires kebab-case strict serde.
    if let Some(scope) = args.scope {
        config.scope = scope;
        changed = true;
        pending.push(format!(
            "  {} scope = {}",
            "✓".green(),
            scope.to_string().cyan()
        ));
    }

    if let Some(behavior) = args.clean_behavior {
        config.clean_behavior = behavior;
        changed = true;
        pending.push(format!(
            "  {} clean-behavior = {}",
            "✓".green(),
            behavior.to_string().cyan()
        ));
    }

    if let Some(mode) = args.default_mode {
        config.default_mode = mode;
        changed = true;
        pending.push(format!(
            "  {} default-mode = {}",
            "✓".green(),
            mode.to_string().cyan()
        ));
    }

    if let Some(min_mb) = args.min_size_mb {
        config.min_size_mb = min_mb;
        changed = true;
        pending.push(format!(
            "  {} min-size-mb = {}",
            "✓".green(),
            min_mb.to_string().cyan()
        ));
    }

    if let Some(ignored_str) = args.ignored_projects {
        let ignored: Vec<String> = ignored_str
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        config.ignored_projects = ignored;
        changed = true;
        pending.push(format!(
            "  {} ignored-projects = {}",
            "✓".green(),
            if config.ignored_projects.is_empty() {
                "(none)".to_string()
            } else {
                config.ignored_projects.join(", ")
            }
            .cyan()
        ));
    }

    // Catch CLI-caused semantic errors (empty dir, overflowing min-size)
    // before touching disk.
    if changed && !invalid {
        if let Err(error) = config.validate() {
            invalid = true;
            eprintln!("  {} Invalid settings: {error}.", "✗".red());
        }
    }

    if invalid {
        std::process::exit(2);
    }
    if changed {
        match config.save() {
            Ok(()) => {
                for line in pending {
                    println!("{line}");
                }
                println!("  {} Settings saved.", "✓".green().bold());
            }
            Err(error) => {
                eprintln!("  {} Failed to save: {error}.", "✗".red().bold());
                std::process::exit(1);
            }
        }
        return;
    }
    println!("No settings specified. Use --projects-dir, --scope, --clean-behavior, --default-mode, --min-size-mb, or --ignored-projects.");
    println!("Run 'deox settings show' to view current settings.");
}

fn reset_settings(args: SettingsResetArgs) {
    if !args.yes {
        use dialoguer::Confirm;
        // Declining (Ok(false)) is a normal cancel with exit 0; only prompt
        // I/O failure (e.g. non-TTY) exits 1.
        match Confirm::new()
            .with_prompt("Reset all settings to defaults?")
            .default(false)
            .interact()
        {
            Ok(true) => {}
            Ok(false) => {
                println!("Cancelled.");
                return;
            }
            Err(error) => {
                eprintln!("Reset cancelled: {error} (use --yes to skip confirmation).");
                std::process::exit(1);
            }
        }
    }

    let config = Config::default();
    match config.save() {
        Ok(()) => {
            println!("  {} Settings reset to defaults.", "✓".green().bold());
        }
        Err(error) => {
            eprintln!("  {} Failed to reset: {error}.", "✗".red().bold());
            std::process::exit(1);
        }
    }
}
