use colored::Colorize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const REPO_OWNER: &str = "BurntToasters";
const REPO_NAME: &str = "deoxidizer";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_RELEASE_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: usize = 4 * 1024 * 1024;
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;

/// Run the self-update check and update flow.
pub fn run_update() {
    println!();
    println!("  {} Checking for updates...", "🔄".bold());

    let release = match fetch_latest_release() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  {} Failed to check for updates: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    let latest_tag = release.tag_name.trim_start_matches('v');
    let current = CURRENT_VERSION;

    if !is_newer(latest_tag, current) {
        println!(
            "  {} deoxidizer is already up to date (v{}).",
            "✓".green().bold(),
            current
        );
        return;
    }

    println!(
        "  New version available: v{} → v{}",
        current.dimmed(),
        latest_tag.green().bold()
    );

    // Determine the asset name for this platform
    let asset_name = platform_asset_name(latest_tag);
    let (platform_os, platform_arch) = platform_tokens();
    let target_checksums_name = format!("SHA256SUMS-{platform_os}-{platform_arch}.txt");

    // Find the download URLs
    let asset_url = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .map(|a| a.browser_download_url.clone());

    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name == target_checksums_name)
        .or_else(|| release.assets.iter().find(|a| a.name == "SHA256SUMS.txt"))
        .map(|a| a.browser_download_url.clone());

    let asset_url = match asset_url {
        Some(url) => url,
        None => {
            eprintln!(
                "  {} No release asset found for this platform: {}",
                "✗".red().bold(),
                asset_name
            );
            eprintln!("  Available assets:");
            for a in &release.assets {
                eprintln!("    - {}", a.name);
            }
            std::process::exit(1);
        }
    };

    println!("  Downloading {}...", asset_name);

    let checksums_url = match checksums_url {
        Some(url) => url,
        None => {
            eprintln!(
                "  {} No platform or global SHA256SUMS manifest found in release. Aborting update.",
                "✗".red().bold()
            );
            std::process::exit(1);
        }
    };

    if !is_allowed_download_url(&asset_url) || !is_allowed_download_url(&checksums_url) {
        eprintln!(
            "  {} Release contains an untrusted download URL. Aborting update.",
            "✗".red().bold()
        );
        std::process::exit(1);
    }

    // Download the binary asset
    let asset_bytes = match download_bytes(&asset_url, MAX_ASSET_BYTES) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("  {} Download failed: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    println!("  Verifying SHA256 checksum...");
    let checksums_bytes = match download_bytes(&checksums_url, MAX_CHECKSUM_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "  {} Could not download checksums: {}. Aborting update.",
                "✗".red().bold(),
                error
            );
            std::process::exit(1);
        }
    };
    let checksums_str = match std::str::from_utf8(&checksums_bytes) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "  {} SHA256SUMS.txt is not valid UTF-8: {}",
                "✗".red().bold(),
                error
            );
            std::process::exit(1);
        }
    };
    if !verify_sha256(&asset_bytes, &asset_name, checksums_str) {
        eprintln!(
            "  {} SHA256 checksum verification FAILED! Aborting update.",
            "✗".red().bold()
        );
        eprintln!("  The downloaded file may be corrupted or tampered with.");
        std::process::exit(1);
    }
    println!("  {} SHA256 checksum verified.", "✓".green());

    // Extract and replace
    println!("  Installing update...");
    match install_update(&asset_bytes, &asset_name) {
        Ok(()) => {
            println!(
                "  {} Successfully updated to v{}!",
                "✓".green().bold(),
                latest_tag
            );
        }
        Err(e) => {
            eprintln!("  {} Update failed: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    }
    println!();
}

// --- GitHub API types ---

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn fetch_latest_release() -> Result<GithubRelease, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );

    if !is_allowed_download_url(&url) {
        return Err("latest release URL is not HTTPS GitHub API".to_string());
    }

    let response = match http_agent()
        .get(&url)
        .set("User-Agent", &format!("deoxidizer/{}", CURRENT_VERSION))
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(404, _)) => {
            return Err(format!(
                "No releases found on GitHub for {}/{} (no releases published yet)",
                REPO_OWNER, REPO_NAME
            ));
        }
        Err(e) => return Err(format!("HTTP request failed: {e}")),
    };

    let bytes = read_limited(response.into_reader(), MAX_RELEASE_JSON_BYTES)?;
    let release: GithubRelease =
        serde_json::from_slice(&bytes).map_err(|e| format!("Failed to parse release JSON: {e}"))?;

    Ok(release)
}

fn download_bytes(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    if !is_allowed_download_url(url) {
        return Err("download URL is not HTTPS GitHub-hosted content".to_string());
    }
    let response = http_agent()
        .get(url)
        .set("User-Agent", &format!("deoxidizer/{}", CURRENT_VERSION))
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    read_limited(response.into_reader(), max_bytes)
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .build()
}

fn read_limited<R: Read>(mut reader: R, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read failed: {e}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("response exceeds {max_bytes} byte limit"));
    }
    Ok(bytes)
}

fn is_allowed_download_url(url: &str) -> bool {
    url.starts_with("https://api.github.com/")
        || url.starts_with("https://github.com/")
        || url.starts_with("https://objects.githubusercontent.com/")
}

/// Compare semver strings: returns true if `latest` > `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let parts: Vec<&str> = s.split('.').collect();
        let major = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch_str = parts.get(2).unwrap_or(&"0");
        // Strip pre-release suffixes for comparison
        let patch_num = patch_str
            .split('-')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        (major, minor, patch_num)
    };
    parse(latest) > parse(current)
}

/// Determine the expected release asset name for the current platform.
fn platform_asset_name(version: &str) -> String {
    let (os, arch) = platform_tokens();

    let ext = if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    };

    format!("deoxidizer-v{version}-{os}-{arch}.{ext}")
}

fn platform_tokens() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    (os, arch)
}

/// Verify the SHA256 checksum of downloaded bytes against a checksums manifest.
fn verify_sha256(data: &[u8], asset_name: &str, checksums: &str) -> bool {
    // Compute the SHA256 of the downloaded data
    let mut hasher = Sha256::new();
    hasher.update(data);
    let computed = hex::encode(hasher.finalize());

    let mut expected = None;
    for line in checksums.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() == 2 && parts[1].trim_start_matches('*') == asset_name {
            if expected.is_some()
                || parts[0].len() != 64
                || !parts[0]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return false;
            }
            expected = Some(parts[0]);
        }
    }

    expected.is_some_and(|value| value.eq_ignore_ascii_case(&computed))
}

/// Extract the update and replace the current binary.
fn install_update(archive_bytes: &[u8], asset_name: &str) -> Result<(), String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("deoxidizer-update-")
        .tempdir()
        .map_err(|e| format!("Failed to create secure temp dir: {e}"))?;

    let default_binary_name = if cfg!(target_os = "windows") {
        "deoxidizer.exe"
    } else {
        "deoxidizer"
    };
    let current_exe = std::env::current_exe().ok();
    let binary_name = current_exe
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| *name == default_binary_name || *name == "deox.exe" || *name == "deox")
        .unwrap_or(default_binary_name);

    if asset_name.ends_with(".tar.gz") {
        // Extract tar.gz
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive_bytes));
        extract_tar(decoder, temp_dir.path())?;
    } else if asset_name.ends_with(".zip") {
        extract_zip(archive_bytes, temp_dir.path())?;
    } else {
        return Err(format!("Unknown archive format: {asset_name}"));
    }

    // Find the binary in the extracted files
    let new_binary = find_binary_in_dir(temp_dir.path(), binary_name)
        .ok_or_else(|| format!("Could not find '{binary_name}' in extracted archive"))?;

    // On Windows, also update sibling binary (deox.exe <-> deoxidizer.exe)
    if cfg!(target_os = "windows") {
        if let Some(current_exe) = current_exe {
            if let Some(parent) = current_exe.parent() {
                let sibling_name = if current_exe.file_name() == Some("deox.exe".as_ref()) {
                    "deoxidizer.exe"
                } else {
                    "deox.exe"
                };
                let sibling_dest = parent.join(sibling_name);
                if fs::symlink_metadata(&sibling_dest).is_ok() {
                    let new_sibling = find_binary_in_dir(temp_dir.path(), sibling_name)
                        .ok_or_else(|| format!("Could not find '{sibling_name}' in update"))?;
                    fs::copy(&new_sibling, &sibling_dest)
                        .map_err(|e| format!("Failed to replace {sibling_name}: {e}"))?;
                }
            }
        }
    }

    // Replace the current running binary
    self_replace::self_replace(&new_binary)
        .map_err(|e| format!("Failed to replace binary: {e}"))?;

    Ok(())
}

/// Recursively find a binary by name in a directory.
fn find_binary_in_dir(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file()
            && !entry.file_type().is_symlink()
            && entry.file_name().to_string_lossy() == name
        {
            return Some(entry.path().to_path_buf());
        }
    }
    None
}

fn validate_archive_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err(format!("archive contains absolute path {}", path.display()));
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => safe.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("archive contains unsafe path {}", path.display()));
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err("archive contains an empty path".to_string());
    }
    Ok(safe)
}

fn extract_tar<R: Read>(reader: R, destination: &Path) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| format!("Failed to read tar entries: {error}"))?;
    let mut extracted_bytes = 0u64;

    for entry in entries {
        let mut entry = entry.map_err(|error| format!("Failed to read tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("Invalid tar path: {error}"))?
            .to_path_buf();
        let safe_path = validate_archive_path(&path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(format!("archive contains link entry {}", path.display()));
        }
        let output = destination.join(&safe_path);
        if !output.starts_with(destination) {
            return Err(format!(
                "archive path escaped extraction directory: {}",
                path.display()
            ));
        }

        if entry_type.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Failed to create {}: {error}", output.display()))?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(format!(
                "archive contains unsupported entry {}",
                path.display()
            ));
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.header().size().map_err(|error| error.to_string())?)
            .ok_or_else(|| "archive extracted size overflow".to_string())?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err("archive exceeds extracted size limit".to_string());
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        }
        entry
            .unpack(&output)
            .map_err(|error| format!("Failed to extract {}: {error}", path.display()))?;
    }
    Ok(())
}

fn extract_zip(archive_bytes: &[u8], destination: &Path) -> Result<(), String> {
    let cursor = std::io::Cursor::new(archive_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|error| format!("Failed to open zip: {error}"))?;
    let mut extracted_bytes = 0u64;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Failed to read zip entry: {error}"))?;
        let raw_name = entry.name().to_string();
        if raw_name.contains('\\') {
            return Err(format!("archive contains unsafe path {raw_name}"));
        }
        let path = Path::new(&raw_name);
        let safe_path = validate_archive_path(path)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("archive contains link entry {raw_name}"));
        }
        let output = destination.join(&safe_path);
        if !output.starts_with(destination) {
            return Err(format!(
                "archive path escaped extraction directory: {raw_name}"
            ));
        }

        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Failed to create {}: {error}", output.display()))?;
            continue;
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "archive extracted size overflow".to_string())?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err("archive exceeds extracted size limit".to_string());
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        }
        let mut output_file = fs::File::create(&output)
            .map_err(|error| format!("Failed to create {}: {error}", output.display()))?;
        io::copy(&mut entry, &mut output_file)
            .map_err(|error| format!("Failed to extract {raw_name}: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_requires_exact_asset_entry() {
        let data = b"release";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = hex::encode(hasher.finalize());
        let manifest = format!("{digest}  deoxidizer-v1-linux-x86_64.tar.gz\n");

        assert!(verify_sha256(
            data,
            "deoxidizer-v1-linux-x86_64.tar.gz",
            &manifest
        ));
        assert!(!verify_sha256(data, "other.tar.gz", &manifest));
        assert!(!verify_sha256(
            data,
            "deoxidizer-v1-linux-x86_64.tar.gz",
            "not-a-checksum\n"
        ));
        assert!(!verify_sha256(
            data,
            "deoxidizer-v1-linux-x86_64.tar.gz",
            &format!("{manifest}{manifest}")
        ));
    }

    #[test]
    fn download_urls_require_github_https() {
        assert!(is_allowed_download_url(
            "https://api.github.com/repos/BurntToasters/deoxidizer/releases/latest"
        ));
        assert!(is_allowed_download_url(
            "https://github.com/BurntToasters/deoxidizer/releases/download/v1/a.tar.gz"
        ));
        assert!(!is_allowed_download_url(
            "http://github.com/BurntToasters/deoxidizer/releases/latest"
        ));
        assert!(!is_allowed_download_url("https://evil.example/a.tar.gz"));
    }

    #[test]
    fn archive_paths_reject_escape_components() {
        assert!(validate_archive_path(Path::new("deoxidizer")).is_ok());
        assert!(validate_archive_path(Path::new("../outside")).is_err());
        assert!(validate_archive_path(Path::new("/absolute")).is_err());
    }
}
