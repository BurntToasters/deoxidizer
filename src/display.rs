use crate::cleaner::{CleanMode, CleanResult};
use crate::project::{DiscoveredProject, ProjectKind};
use colored::*;
use console::{measure_text_width, pad_str, truncate_str, Alignment};
use humansize::{format_size, BINARY};
use std::path::Path;
use std::time::UNIX_EPOCH;

const NAME_WIDTH: usize = 20;

/// Render a path for humans: home directory shown as `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
        }
    }
    path.display().to_string()
}

fn cell(text: &str, width: usize, align: Alignment) -> String {
    let text = if measure_text_width(text) > width {
        truncate_str(text, width, "…").into_owned()
    } else {
        text.to_string()
    };
    pad_str(&text, width, align, None).into_owned()
}

fn age_label(project: &DiscoveredProject) -> String {
    project
        .last_modified
        .and_then(|t| t.elapsed().ok())
        .map(|d| match d.as_secs() / 86400 {
            0 => "today".to_string(),
            1 => "1 day ago".to_string(),
            days => format!("{days} days ago"),
        })
        .unwrap_or_else(|| "—".to_string())
}

fn size_or_dash(bytes: u64) -> String {
    if bytes > 0 {
        format_size(bytes, BINARY)
    } else {
        "—".to_string()
    }
}

fn kind_label(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::TauriApp => "Tauri",
        ProjectKind::RustProject => "Rust",
    }
}

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

    println!();
    println!(
        "  {} {} {} {} {} {}  {}  {}",
        cell("#", 4, Alignment::Left).dimmed(),
        cell("Project", NAME_WIDTH, Alignment::Left).bold(),
        cell("Type", 6, Alignment::Left).bold(),
        cell("Total", 11, Alignment::Right).bold(),
        cell("Debug", 11, Alignment::Right).bold(),
        cell("Release", 11, Alignment::Right).bold(),
        cell("Last Build", 12, Alignment::Left).bold(),
        "Path".bold(),
    );
    let rule = "─".repeat(90);
    println!("  {}", rule.dimmed());

    let mut total_size = 0u64;
    for (i, project) in projects.iter().enumerate() {
        let (debug, release) = project
            .breakdown
            .as_ref()
            .map(|b| (size_or_dash(b.debug_size), size_or_dash(b.release_size)))
            .unwrap_or_else(|| ("—".to_string(), "—".to_string()));
        let kind = cell(kind_label(&project.kind), 6, Alignment::Left);
        let kind = match project.kind {
            ProjectKind::TauriApp => kind.magenta(),
            ProjectKind::RustProject => kind.normal(),
        };
        println!(
            "  {} {} {} {} {} {}  {}  {}",
            cell(&(i + 1).to_string(), 4, Alignment::Left).dimmed(),
            cell(&project.name, NAME_WIDTH, Alignment::Left).cyan(),
            kind,
            cell(
                &format_size(project.artifact_size, BINARY),
                11,
                Alignment::Right
            )
            .yellow()
            .bold(),
            cell(&debug, 11, Alignment::Right),
            cell(&release, 11, Alignment::Right),
            cell(&age_label(project), 12, Alignment::Left).dimmed(),
            display_path(&project.path).dimmed(),
        );
        total_size = total_size.saturating_add(project.artifact_size);
    }

    println!("  {}", rule.dimmed());
    println!(
        "  {} Total artifact size: {}",
        "✨".bold(),
        format_size(total_size, BINARY).green().bold(),
    );
    println!();
}

/// JSON representation of one project (stable field names for scripts).
pub fn project_json(project: &DiscoveredProject) -> serde_json::Value {
    let breakdown = project.breakdown.as_ref().map(|b| {
        serde_json::json!({
            "debug": b.debug_size,
            "release": b.release_size,
            "incremental": b.incremental_size,
            "deps": b.deps_size,
            "other": b.other_size,
        })
    });
    serde_json::json!({
        "name": project.name,
        "path": project.path,
        "kind": match project.kind {
            ProjectKind::TauriApp => "tauri",
            ProjectKind::RustProject => "rust",
        },
        "artifact_dir": project.artifact_dir,
        "artifact_size": project.artifact_size,
        "last_modified_unix": project
            .last_modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        "breakdown": breakdown,
        "members": project.members,
    })
}

/// Print scan results as JSON on stdout.
pub fn print_scan_json(projects: &[DiscoveredProject]) {
    let total: u64 = projects
        .iter()
        .fold(0u64, |acc, p| acc.saturating_add(p.artifact_size));
    let value = serde_json::json!({
        "projects": projects.iter().map(project_json).collect::<Vec<_>>(),
        "total_bytes": total,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

/// Print a single clean result.
pub fn print_clean_result(project: &DiscoveredProject, result: &CleanResult) {
    let label = format!("{} ({})", project.name, display_path(&project.path));
    match result {
        CleanResult::Cleaned { bytes_freed } => {
            println!(
                "  {} {} — freed {}",
                "✓".green().bold(),
                label.cyan(),
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
                label.cyan(),
                format_size(*bytes_freed, BINARY).yellow(),
                message,
            );
        }
        CleanResult::Skipped { reason } => {
            println!(
                "  {} {} — skipped: {}",
                "–".yellow(),
                label,
                reason.dimmed()
            );
        }
        CleanResult::Error { message } => {
            println!(
                "  {} {} — {}: {}",
                "✗".red().bold(),
                label,
                "error".red(),
                message
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
    trashed: bool,
) {
    println!();
    let projects_noun = if cleaned_count == 1 {
        "1 project".to_string()
    } else {
        format!("{cleaned_count} projects")
    };
    let verb = if trashed { "moved to Trash" } else { "freed" };
    let icon = if error_count == 0 {
        "🧹".bold()
    } else {
        "⚠".yellow().bold()
    };
    print!(
        "  {icon} Cleaned {projects_noun} (mode: {mode}), {verb} {}.",
        format_size(total_freed, BINARY).green().bold(),
    );
    match error_count {
        0 => println!(),
        1 => println!(" 1 error."),
        n => println!(" {n} errors."),
    }
    if trashed && total_freed > 0 {
        println!("  Empty the Trash / Recycle Bin to reclaim the disk space.");
    }
    println!();
}

/// Print a detailed inspection of a single project.
pub fn print_inspection(project: &DiscoveredProject) {
    println!();
    println!("  {} {}", "Project:".bold(), project.name.cyan().bold());
    println!("  {} {}", "Path:".bold(), display_path(&project.path));
    println!("  {} {}", "Type:".bold(), project.kind);
    if project.members.len() > 1 || project.members.first() != Some(&project.name) {
        println!("  {} {}", "Packages:".bold(), project.members.join(", "));
    }
    println!(
        "  {} {}",
        "Total Size:".bold(),
        format_size(project.artifact_size, BINARY).yellow().bold()
    );

    if let Some(ref b) = project.breakdown {
        let row = |label: &str, bytes: u64| {
            println!(
                "    {:<34} {:>20}",
                label,
                format!(
                    "{} ({})",
                    format_size(bytes, BINARY),
                    percent_of(bytes, project.artifact_size)
                )
            );
        };
        println!();
        println!("  {}", "Target Breakdown:".bold().underline());
        row("debug/ (all targets)", b.debug_size);
        row("release/ (all targets)", b.release_size);
        if b.other_size > 0 {
            row("other (doc/, custom profiles, ...)", b.other_size);
        }
        println!();
        println!("  {}", "Cache views (overlap the rows above):".bold());
        row("incremental/ (all profiles)", b.incremental_size);
        row("deps/ (all profiles)", b.deps_size);
        println!();
        println!(
            "  {}",
            "Each row is exactly what the matching clean mode frees.".dimmed()
        );
    }

    println!("  {} {}", "Last Build:".bold(), age_label(project));
    println!();
}

/// Format `part / total` as a whole-number percentage for subset rows.
fn percent_of(part: u64, total: u64) -> String {
    if total == 0 {
        return "—".to_string();
    }
    let percent = (u128::from(part) * 100 / u128::from(total)).min(100);
    format!("{percent}%")
}
