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
        return;
    }

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
    println!("  {}", "─".repeat(90).dimmed());

    let mut total_size = 0u64;

    for (i, project) in projects.iter().enumerate() {
        let kind_label = match &project.kind {
            crate::project::ProjectKind::TauriApp => "🦀 Tauri".to_string(),
            crate::project::ProjectKind::RustProject => "🦀 Rust".to_string(),
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
            project.name.cyan(),
            kind_label,
            size_str.yellow().bold(),
            debug_str,
            release_str,
            age_str.dimmed(),
        );

        total_size += project.artifact_size;
    }

    println!("  {}", "─".repeat(90).dimmed());
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
pub fn print_clean_summary(total_freed: u64, cleaned_count: usize, error_count: usize) {
    println!();
    if error_count == 0 {
        println!(
            "  {} Cleaned {} project(s), freed {}.",
            "🧹".bold(),
            cleaned_count,
            format_size(total_freed, BINARY).green().bold(),
        );
    } else {
        println!(
            "  {} Cleaned {} project(s), freed {}. {} error(s).",
            "⚠".yellow().bold(),
            cleaned_count,
            format_size(total_freed, BINARY).green().bold(),
            error_count,
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
            "    {:<20} {:>12}",
            "debug/",
            format_size(b.debug_size, BINARY)
        );
        println!(
            "    {:<20} {:>12}",
            "  ├─ incremental/",
            format_size(b.incremental_size, BINARY)
        );
        println!(
            "    {:<20} {:>12}",
            "  └─ deps/",
            format_size(b.deps_size, BINARY)
        );
        println!(
            "    {:<20} {:>12}",
            "release/",
            format_size(b.release_size, BINARY)
        );
        if b.other_size > 0 {
            println!(
                "    {:<20} {:>12}",
                "other/",
                format_size(b.other_size, BINARY)
            );
        }
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
