use crate::cleaner::{CleanMode, CleanResult};
use crate::project::DiscoveredProject;
use colored::*;
use humansize::{format_size, BINARY};

/// Print the scan results table.
pub fn print_scan_results(projects: &[DiscoveredProject]) {
    if projects.is_empty() {
        println!(
            "{}  No projects with build artifacts found.",
            "ℹ".blue().bold()
        );
        println!(
            "  Run {} to review scope and filters.",
            "deox settings show".green()
        );
        return;
    }

    // Fixed table width for the separator; column format below must stay in
    // sync with it.
    const TABLE_WIDTH: usize = 90;
    const NAME_WIDTH: usize = 20;

    println!();
    // Header
    println!(
        "  {:<4} {:<20} {:<8} {:>12} {:>12} {:>12}  {}",
        "#".dimmed(),
        "Project".bold(),
        "Type".bold(),
        "Total".bold(),
        "Debug".bold(),
        "Release".bold(),
        "Last Build".bold(),
    );
    println!("  {}", "─".repeat(TABLE_WIDTH).dimmed());

    let mut total_size = 0u64;

    for (i, project) in projects.iter().enumerate() {
        let kind_label = match &project.kind {
            crate::project::ProjectKind::TauriApp => "🦀 Tauri".to_string(),
            crate::project::ProjectKind::RustProject => "⚙ Rust".to_string(),
        };

        let debug_str = project
            .breakdown
            .as_ref()
            .map(|b| {
                if b.debug_size > 0 {
                    format_size(b.debug_size, BINARY)
                } else {
                    "—".to_string()
                }
            })
            .unwrap_or_else(|| "—".to_string());

        let release_str = project
            .breakdown
            .as_ref()
            .map(|b| {
                if b.release_size > 0 {
                    format_size(b.release_size, BINARY)
                } else {
                    "—".to_string()
                }
            })
            .unwrap_or_else(|| "—".to_string());

        let age_str = project
            .last_modified
            .and_then(|t| t.elapsed().ok())
            .map(|d| {
                let days = d.as_secs() / 86400;
                if days == 0 {
                    "today".to_string()
                } else if days == 1 {
                    "1 day ago".to_string()
                } else {
                    format!("{days} days ago")
                }
            })
            .unwrap_or_else(|| "—".to_string());

        let size_str = format_size(project.artifact_size, BINARY);

        println!(
            "  {:<4} {:<20} {:<8} {:>12} {:>12} {:>12}  {}",
            format!("{}", i + 1).dimmed(),
            truncate_name(&project.name, NAME_WIDTH).cyan(),
            kind_label,
            size_str.yellow().bold(),
            debug_str,
            release_str,
            age_str.dimmed(),
        );

        total_size = total_size.saturating_add(project.artifact_size);
    }

    println!("  {}", "─".repeat(TABLE_WIDTH).dimmed());
    println!(
        "  {} Total artifact size: {}",
        "✨".bold(),
        format_size(total_size, BINARY).green().bold(),
    );
    println!();
}

/// Print a single clean result.
pub fn print_clean_result(project_name: &str, result: &CleanResult, _mode: &CleanMode) {
    match result {
        CleanResult::Cleaned { bytes_freed } => {
            println!(
                "  {} {} — freed {}",
                "✓".green().bold(),
                project_name.cyan(),
                format_size(*bytes_freed, BINARY).green(),
            );
        }
        CleanResult::Partial {
            bytes_freed,
            message,
        } => {
            println!(
                "  {} {} — freed {} with errors: {}",
                "⚠".yellow().bold(),
                project_name.cyan(),
                format_size(*bytes_freed, BINARY).yellow(),
                message,
            );
        }
        CleanResult::Skipped { reason } => {
            println!(
                "  {} {} — skipped: {}",
                "–".yellow(),
                project_name,
                reason.dimmed(),
            );
        }
        CleanResult::Error { message } => {
            println!(
                "  {} {} — {}: {}",
                "✗".red().bold(),
                project_name,
                "error".red(),
                message,
            );
        }
    }
}

/// Print a summary after cleaning.
pub fn print_clean_summary(
    total_freed: u64,
    cleaned_count: usize,
    error_count: usize,
    mode: &CleanMode,
) {
    println!();
    let projects_noun = if cleaned_count == 1 {
        "1 project".to_string()
    } else {
        format!("{cleaned_count} projects")
    };
    if error_count == 0 {
        println!(
            "  {} Cleaned {} (mode: {}), freed {}.",
            "🧹".bold(),
            projects_noun,
            mode,
            format_size(total_freed, BINARY).green().bold(),
        );
    } else {
        let errors_noun = if error_count == 1 {
            "1 error".to_string()
        } else {
            format!("{error_count} errors")
        };
        println!(
            "  {} Cleaned {} (mode: {}), freed {}. {}.",
            "⚠".yellow().bold(),
            projects_noun,
            mode,
            format_size(total_freed, BINARY).green().bold(),
            errors_noun,
        );
    }
    println!();
}

/// Print a detailed inspection of a single project.
pub fn print_inspection(project: &DiscoveredProject) {
    println!();
    println!("  {} {}", "Project:".bold(), project.name.cyan().bold());
    println!("  {} {}", "Path:".bold(), project.path.display());
    println!("  {} {}", "Type:".bold(), project.kind);
    println!(
        "  {} {}",
        "Total Size:".bold(),
        format_size(project.artifact_size, BINARY).yellow().bold()
    );

    if let Some(ref b) = project.breakdown {
        println!();
        println!("  {}", "Target Breakdown:".bold().underline());
        println!(
            "    {:<30} {:>20}",
            "debug/",
            format_size(b.debug_size, BINARY)
        );
        println!(
            "    {:<30} {:>20}",
            "  ├─ incremental/ (of which)",
            format!(
                "{} ({})",
                format_size(b.incremental_size, BINARY),
                percent_of(b.incremental_size, project.artifact_size)
            )
        );
        println!(
            "    {:<30} {:>20}",
            "  └─ deps/ (of which)",
            format!(
                "{} ({})",
                format_size(b.deps_size, BINARY),
                percent_of(b.deps_size, project.artifact_size)
            )
        );
        println!(
            "    {:<30} {:>20}",
            "release/",
            format_size(b.release_size, BINARY)
        );
        if b.other_size > 0 {
            println!(
                "    {:<30} {:>20}",
                "other/",
                format_size(b.other_size, BINARY)
            );
        }
        println!();
        println!(
            "  {}",
            "Note: incremental/ and deps/ are subsets already counted above; do not sum.".dimmed()
        );
    }

    if let Some(elapsed) = project.last_modified.and_then(|t| t.elapsed().ok()) {
        let days = elapsed.as_secs() / 86400;
        let age = if days == 0 {
            "today".to_string()
        } else if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{days} days ago")
        };
        println!("  {} {}", "Last Build:".bold(), age);
    }
    println!();
}

/// Truncate a project name to `max` display columns, appending `…`.
///
/// NOTE: char-count based, not `unicode-width`: `console` is only a
/// transitive dependency via `dialoguer`, so measuring grapheme/display
/// width would require a new direct dependency. CJK/emoji names may
/// therefore occupy more terminal columns than `max`.
fn truncate_name(name: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if name.chars().count() <= max {
        return name.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    format!(
        "{}…",
        name.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

/// Format `part / total` as a whole-number percentage for subset rows.
fn percent_of(part: u64, total: u64) -> String {
    if total == 0 {
        return "—".to_string();
    }
    let percent = (u128::from(part) * 100 / u128::from(total)).min(100);
    format!("{percent}%")
}
