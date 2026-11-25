use colored::Colorize;
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const REPO_OWNER: &str = "BurntToasters";
const REPO_NAME: &str = "deoxidizer";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_RELEASE_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: usize = 4 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 64 * 1024;
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RELEASE_PROOF_BYTES: u64 = 1024 * 1024;
const MAX_REDIRECT_HOPS: usize = 3;
const RELEASE_PROOF_NAME: &str = "DEOX-RELEASE-PROOF.json";
const PENDING_UPDATE_DIRECTORY: &str = ".deoxidizer-update-pending";
const UPDATE_READ_TIMEOUT_CAP: Duration = Duration::from_secs(30);
/// Overall network deadline for the update flow (fetch + downloads).
///
/// A single `Instant` deadline created at the start of `run_update` is
/// threaded through request creation and response reads. Each ureq agent uses
/// `.timeout(remaining)` for the overall request, plus a connect cap no larger
/// than the remaining budget. ureq 2 documents that this deadline covers
/// connection, headers, and response body, but a blocked system DNS lookup
/// cannot be interrupted or bounded through ureq's available API. The
/// request may therefore exceed 150 seconds by the duration of that lookup;
/// after DNS returns, the expired connect deadline aborts the request.
/// `.timeout_connect()` takes precedence over `.timeout()`, so its value is
/// explicitly capped by the remaining budget. Redirects stay disabled in
/// ureq and each validated hop reuses this same flow deadline. Single-attempt,
/// no-retry: timeouts fail closed.
const UPDATE_DEADLINE: Duration = Duration::from_secs(150);

/// Install outcome that distinguishes a clean failure from a partial
/// update where the running binary was already replaced but the sibling
/// alias sync failed (version skew). The main binary is new in the latter
/// case, so the caller must say so explicitly instead of reporting a
/// generic failure.
#[derive(Debug)]
enum InstallFailure {
    Failed(String),
    MainUpdatedSiblingFailed(String),
}

/// Production extraction report used by release-time archive verification.
/// The proof's platform-specific native fields remain release-tool policy;
/// the updater guarantees only that proof artifact hashes match the exact
/// binaries accepted by its own extractor.
#[doc(hidden)]
#[derive(Debug)]
pub struct ReleaseArchiveReport {
    pub binaries: BTreeMap<String, String>,
    pub proof_json: Option<serde_json::Value>,
}

impl std::fmt::Display for InstallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) | Self::MainUpdatedSiblingFailed(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// Run the self-update check and update flow.
///
/// Order is load-bearing: manifest -> asset -> hash -> install. The small
/// checksum manifest and its detached signature are fetched and verified
/// first so a tampered manifest aborts before the large asset download;
/// the asset is hashed against the verified manifest before install.
///
/// With `check_only`, report whether a newer release exists and stop.
pub fn run_update(check_only: bool) {
    println!();
    println!("  {} Checking for updates...", "🔄".bold());

    // A package remains beside the installed executable until both invokers
    // have been replaced. Recover it before consulting the latest version so
    // a same-version retry (or a launch through the stale sibling) can finish.
    if !check_only {
        match recover_pending_update() {
            Ok(true) => {
                println!(
                    "  {} Recovered the pending update for both invokers.",
                    "✓".green().bold()
                );
                return;
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!(
                    "  {} Pending update recovery failed: {error}",
                    "✗".red().bold()
                );
                std::process::exit(1);
            }
        }
    }

    // Single overall deadline for fetch + all downloads. Single-attempt,
    // no-retry: on expiry every network read aborts fail-closed.
    let deadline = Instant::now() + UPDATE_DEADLINE;
    let release = match fetch_latest_release(deadline) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  {} Failed to check for updates: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    let trimmed_tag = release.tag_name.trim();
    let latest_tag = trimmed_tag.strip_prefix('v').unwrap_or(trimmed_tag);
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
    if check_only {
        println!("  Run 'deox --update' to install it.");
        return;
    }

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

    let checksum_signature_name = checksums_url
        .as_ref()
        .and_then(|url| url.rsplit('/').next().map(|name| format!("{name}.minisig")));
    let checksum_signature_url = checksum_signature_name.as_ref().and_then(|name| {
        release
            .assets
            .iter()
            .find(|asset| &asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    });

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
    let checksum_signature_url = match checksum_signature_url {
        Some(url) => url,
        None => {
            eprintln!(
                "  {} No detached signature found for checksum manifest. Aborting update.",
                "✗".red().bold()
            );
            std::process::exit(1);
        }
    };

    if !is_allowed_download_url(&asset_url)
        || !is_allowed_download_url(&checksums_url)
        || !is_allowed_download_url(&checksum_signature_url)
    {
        eprintln!(
            "  {} Release contains an untrusted download URL. Aborting update.",
            "✗".red().bold()
        );
        std::process::exit(1);
    }

    // Fetch the small checksum manifest and its signature first and verify
    // them before downloading the large asset, to avoid a wasted download.
    println!("  Downloading checksums...");
    let checksums_bytes = match download_bytes(&checksums_url, MAX_CHECKSUM_BYTES, deadline) {
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
    let signature_bytes =
        match download_bytes(&checksum_signature_url, MAX_SIGNATURE_BYTES, deadline) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!(
                    "  {} Could not download checksum signature: {error}",
                    "✗".red().bold()
                );
                std::process::exit(1);
            }
        };
    if let Err(error) = verify_signed_manifest(&checksums_bytes, &signature_bytes) {
        eprintln!(
            "  {} Checksum signature verification FAILED: {error}",
            "✗".red().bold()
        );
        std::process::exit(1);
    }
    println!("  {} Checksum manifest signature verified.", "✓".green());
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

    println!("  Downloading {}...", asset_name);

    // Download the binary asset only after the manifest is authenticated.
    let asset_bytes = match download_bytes(&asset_url, MAX_ASSET_BYTES, deadline) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("  {} Download failed: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
    };

    println!("  Verifying SHA256 checksum...");
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
    match install_update(
        &asset_bytes,
        &asset_name,
        &checksums_bytes,
        &signature_bytes,
    ) {
        Ok(()) => {
            println!(
                "  {} Successfully updated to v{}!",
                "✓".green().bold(),
                latest_tag
            );
        }
        Err(InstallFailure::Failed(e)) => {
            eprintln!("  {} Update failed: {}", "✗".red().bold(), e);
            std::process::exit(1);
        }
        Err(InstallFailure::MainUpdatedSiblingFailed(e)) => {
            eprintln!(
                "  {} Main binary updated to v{} but sibling sync failed: {}. Re-run the update or copy the sibling manually.",
                "⚠".yellow().bold(),
                latest_tag,
                e
            );
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
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Format HTTP failures with an explicit rate-limit hint for 403/429.
///
/// Reads `Retry-After` / `X-RateLimit-Reset` from the response when present
/// (parsed from the error's response headers, falling back to the error
/// string), otherwise emits a generic retry-hint. Single-attempt,
/// no-retry: callers surface the hint but never loop.
fn http_error_message(error: ureq::Error) -> String {
    match &error {
        ureq::Error::Status(403, response) | ureq::Error::Status(429, response) => {
            let retry_after = response
                .header("retry-after")
                .map(str::to_string)
                .or_else(|| {
                    let text = error.to_string();
                    parse_header_hint(&text, "retry-after")
                });
            let reset = response
                .header("x-ratelimit-reset")
                .map(str::to_string)
                .or_else(|| {
                    let text = error.to_string();
                    parse_header_hint(&text, "x-ratelimit-reset")
                });
            let mut hint = String::from("rate limited; retry later");
            if let Some(value) = retry_after {
                hint.push_str(&format!(" (Retry-After: {value})"));
            } else if let Some(value) = reset {
                hint.push_str(&format!(" (X-RateLimit-Reset: {value})"));
            } else {
                hint.push_str(" (no Retry-After header; back off before retrying)");
            }
            format!("HTTP request failed: {error} [{hint}]")
        }
        _ => format!("HTTP request failed: {error}"),
    }
}

/// Best-effort extraction of a header hint from an error string when the
/// typed response headers are unavailable.
fn parse_header_hint(text: &str, header: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let needle = format!("{header}:");
    let start = lower.find(&needle)?;
    let value = text[start + needle.len()..]
        .split([',', '\n', ';'])
        .next()?
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Early-reject when `Content-Length` is present and exceeds `max_bytes`,
/// before any body bytes are read.
fn check_content_length(response: &ureq::Response, max_bytes: usize) -> Result<(), String> {
    if let Some(value) = response.header("content-length") {
        if let Ok(declared) = value.trim().parse::<u64>() {
            if declared > max_bytes as u64 {
                return Err(format!(
                    "response declares {declared} bytes which exceeds {max_bytes} byte limit"
                ));
            }
        }
    }
    Ok(())
}

fn fetch_latest_release(deadline: Instant) -> Result<GithubRelease, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );

    if !is_allowed_download_url(&url) {
        return Err("latest release URL is not HTTPS GitHub API".to_string());
    }

    let response =
        match request_with_redirect_validation(&url, Some("application/vnd.github+json"), deadline)
        {
            Err(blocked) => return Err(blocked),
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => match e {
                ureq::Error::Status(404, _) => {
                    return Err(format!(
                        "No releases found on GitHub for {}/{} (no releases published yet)",
                        REPO_OWNER, REPO_NAME
                    ));
                }
                ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => {
                    return Err(http_error_message(e));
                }
                _ => return Err(format!("HTTP request failed: {e}")),
            },
        };

    check_content_length(&response, MAX_RELEASE_JSON_BYTES)?;
    let bytes = read_limited(response.into_reader(), MAX_RELEASE_JSON_BYTES, deadline)?;
    let release: GithubRelease =
        serde_json::from_slice(&bytes).map_err(|e| format!("Failed to parse release JSON: {e}"))?;

    // `releases/latest` must never resolve to a draft or prerelease; the
    // API normally filters those, so seeing one means an unexpected
    // response that must fail closed rather than install pre-release code.
    if release.draft {
        return Err("latest release is a draft; refusing to update".to_string());
    }
    if release.prerelease {
        return Err("latest release is a prerelease; refusing to update".to_string());
    }
    if release.tag_name.trim().is_empty() {
        return Err("latest release is missing its version tag; refusing to update".to_string());
    }
    if release.assets.is_empty() {
        return Err(
            "latest release contains no downloadable assets; refusing to update".to_string(),
        );
    }

    Ok(release)
}

fn download_bytes(url: &str, max_bytes: usize, deadline: Instant) -> Result<Vec<u8>, String> {
    let response = match request_with_redirect_validation(url, None, deadline) {
        Err(blocked) => return Err(blocked),
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => match e {
            ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => {
                return Err(http_error_message(e));
            }
            _ => return Err(format!("Download failed: {e}")),
        },
    };

    check_content_length(&response, max_bytes)?;
    read_limited(response.into_reader(), max_bytes, deadline)
}

/// Perform a GET with automatic redirects disabled and re-validate every hop.
///
/// `http_agent(deadline)` sets `redirects(0)` so ureq never follows a cross-host
/// redirect on its own. Each `Location` target must pass
/// `is_allowed_download_url` before it is followed (relative targets fail
/// closed), and the final `response.get_url()` is asserted against the same
/// allowlist. At most `MAX_REDIRECT_HOPS` redirects are followed; anything
/// else aborts. Signature plus SHA256 verification remains the primary
/// authenticity check.
fn request_with_redirect_validation(
    url: &str,
    accept: Option<&str>,
    deadline: Instant,
) -> Result<Result<ureq::Response, ureq::Error>, String> {
    if !is_allowed_download_url(url) {
        return Err("download URL is not HTTPS GitHub-hosted content".to_string());
    }
    let mut current_url = url.to_string();
    for _ in 0..=MAX_REDIRECT_HOPS {
        if Instant::now() >= deadline {
            return Err("update timed out: overall 150s deadline exceeded".to_string());
        }
        if !is_allowed_download_url(&current_url) {
            return Err("redirect target is not HTTPS GitHub-hosted content".to_string());
        }
        let agent = http_agent(deadline)?;
        let mut request = agent
            .get(&current_url)
            .set("User-Agent", &format!("deoxidizer/{}", CURRENT_VERSION));
        if let Some(accept) = accept {
            request = request.set("Accept", accept);
        }
        let response = match request.call() {
            Ok(resp) => resp,
            Err(_) if Instant::now() >= deadline => {
                return Err("update timed out: overall 150s deadline exceeded".to_string())
            }
            Err(e) => return Ok(Err(e)),
        };
        if Instant::now() >= deadline {
            return Err("update timed out: overall 150s deadline exceeded".to_string());
        }
        if (300..399).contains(&response.status()) {
            let location = response
                .header("location")
                .ok_or_else(|| "redirect without location header".to_string())?
                .to_string();
            if !is_allowed_download_url(&location) {
                return Err("redirect target is not HTTPS GitHub-hosted content".to_string());
            }
            current_url = location;
            continue;
        }
        if !is_allowed_download_url(response.get_url()) {
            return Err("final download URL is not HTTPS GitHub-hosted content".to_string());
        }
        return Ok(Ok(response));
    }
    Err("too many redirects".to_string())
}

fn timeout_for_budget(deadline: Instant, cap: Duration) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining.min(cap))
}

fn http_agent(deadline: Instant) -> Result<ureq::Agent, String> {
    // Proxy environment variables (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`)
    // are honored so updates work behind corporate proxies. A proxy cannot
    // forge an update: TLS is verified end to end, every URL is allowlisted,
    // and authenticity rests on the pinned signature plus SHA256. ureq 2's
    // `.timeout()` enforces the remaining request deadline across headers and
    // body reads. A blocked synchronous system DNS lookup cannot be
    // interrupted or bounded through ureq and may delay return by its full
    // duration; an expired connect deadline aborts once DNS returns. Connect
    // timeout takes precedence over the overall timeout and is capped below.
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("update timed out: overall 150s deadline exceeded".to_string());
    }
    let connect_timeout = timeout_for_budget(deadline, Duration::from_secs(10))
        .ok_or_else(|| "update timed out: overall 150s deadline exceeded".to_string())?;
    let read_timeout = timeout_for_budget(deadline, UPDATE_READ_TIMEOUT_CAP)
        .ok_or_else(|| "update timed out: overall 150s deadline exceeded".to_string())?;
    let write_timeout = timeout_for_budget(deadline, Duration::from_secs(30))
        .ok_or_else(|| "update timed out: overall 150s deadline exceeded".to_string())?;
    // Keep read/write phase budgets derived from the same flow deadline;
    // ureq's overall timeout takes precedence for those phases. The connect
    // cap remains independently active because ureq gives it precedence.
    Ok(ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout(remaining)
        .timeout_connect(connect_timeout)
        .timeout_read(read_timeout)
        .timeout_write(write_timeout)
        .redirects(0)
        .build())
}

fn read_limited<R: Read>(
    mut reader: R,
    max_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err("update timed out: overall 150s deadline exceeded".to_string());
        }
        match reader.read(&mut chunk) {
            Ok(0) if Instant::now() >= deadline => {
                return Err("update timed out: overall 150s deadline exceeded".to_string())
            }
            Ok(0) => break,
            Ok(n) => {
                if bytes.len().saturating_add(n) > max_bytes {
                    return Err(format!("response exceeds {max_bytes} byte limit"));
                }
                bytes.extend_from_slice(&chunk[..n]);
                if Instant::now() >= deadline {
                    return Err("update timed out: overall 150s deadline exceeded".to_string());
                }
            }
            Err(_) if Instant::now() >= deadline => {
                return Err("update timed out: overall 150s deadline exceeded".to_string())
            }
            Err(e) => return Err(format!("Read failed: {e}")),
        }
    }
    Ok(bytes)
}

/// Allowlist for updater download URLs.
///
/// GitHub serves release asset bytes from signed CDN URLs on
/// `release-assets.githubusercontent.com` (formerly
/// `objects.githubusercontent.com`); those prefixes are intentionally open
/// because they cannot authenticate content on their own. Authenticity
/// comes from the detached signature plus SHA256 verification
/// (`verify_signed_manifest` + `verify_sha256`); redirect targets under this
/// prefix are additionally re-validated hop-by-hop by
/// `request_with_redirect_validation`.
fn is_allowed_download_url(url: &str) -> bool {
    url.starts_with("https://api.github.com/repos/BurntToasters/deoxidizer/")
        || url.starts_with("https://github.com/BurntToasters/deoxidizer/releases/download/")
        || url.starts_with("https://release-assets.githubusercontent.com/")
        || url.starts_with("https://objects.githubusercontent.com/")
}

/// Compare semver strings: returns true if `latest` > `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => false,
    }
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

/// Verify the checksum manifest's raw Minisign detached signature against the
/// repository-pinned public key.
fn verify_signed_manifest(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    crate::minisign::verify_detached(manifest, signature)
}

/// Cross-check hook for release tooling (`examples/verify_release_manifest.rs`):
/// runs the same in-process verifier `deox --update` uses, so release-time
/// acceptance matches runtime acceptance. Not part of the public API.
#[doc(hidden)]
pub fn verify_release_manifest(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    verify_release_signature(manifest, signature)
}

/// Cross-check hook for release tooling to verify raw Minisign signatures on
/// arbitrary release payloads using the exact key and policy used by updates.
#[doc(hidden)]
pub fn verify_release_signature(data: &[u8], signature: &[u8]) -> Result<(), String> {
    crate::minisign::verify_detached(data, signature)
}

/// Reuse the updater's production extraction rules for release-time archive
/// checks. Additional safe regular files are allowed. If the archive embeds
/// the native proof, its artifact hashes must bind to these exact extracted
/// binaries before the proof is returned to the release tool.
#[doc(hidden)]
pub fn verify_release_archive(
    archive_bytes: &[u8],
    asset_name: &str,
) -> Result<ReleaseArchiveReport, String> {
    let (_temp_dir, report) = inspect_release_archive(archive_bytes, asset_name)?;
    Ok(report)
}

fn inspect_release_archive(
    archive_bytes: &[u8],
    asset_name: &str,
) -> Result<(tempfile::TempDir, ReleaseArchiveReport), String> {
    if !asset_name.ends_with(".tar.gz") && !asset_name.ends_with(".zip") {
        return Err(format!("unknown archive format: {asset_name}"));
    }
    if asset_name.is_empty()
        || asset_name.contains('/')
        || asset_name.contains('\\')
        || asset_name == "."
        || asset_name == ".."
    {
        return Err("archive name is not a safe file name".to_string());
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("deoxidizer-update-")
        .tempdir()
        .map_err(|error| format!("Failed to create secure temp dir: {error}"))?;
    extract_update_archive(archive_bytes, asset_name, temp_dir.path())?;

    let (primary_name, alias_name) = archive_binary_names_for_asset(asset_name)?;
    let primary = find_unique_binary(temp_dir.path(), &primary_name)?;
    let alias = find_unique_binary(temp_dir.path(), &alias_name)?;
    prepare_extracted_binary(&primary, asset_name)?;
    prepare_extracted_binary(&alias, asset_name)?;

    let mut binaries = BTreeMap::new();
    binaries.insert(primary_name.clone(), sha256_file(&primary)?);
    binaries.insert(alias_name.clone(), sha256_file(&alias)?);

    let proof_candidates = find_entries_named(temp_dir.path(), RELEASE_PROOF_NAME)?;
    let proof_json = match proof_candidates.as_slice() {
        [] => None,
        [proof_path] => {
            if proof_path.parent() != Some(temp_dir.path()) {
                return Err(format!(
                    "release proof must be at the archive root: {RELEASE_PROOF_NAME}"
                ));
            }
            let metadata = fs::symlink_metadata(proof_path)
                .map_err(|error| format!("Failed to inspect release proof: {error}"))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("release proof must be a regular file".to_string());
            }
            if metadata.len() > MAX_RELEASE_PROOF_BYTES {
                return Err("release proof exceeds 1 MiB limit".to_string());
            }
            let bytes = fs::read(proof_path)
                .map_err(|error| format!("Failed to read release proof: {error}"))?;
            Some(validate_release_proof(&bytes, &binaries)?)
        }
        _ => return Err("archive contains duplicate release proof entries".to_string()),
    };

    Ok((
        temp_dir,
        ReleaseArchiveReport {
            binaries,
            proof_json,
        },
    ))
}

fn archive_binary_names_for_asset(asset_name: &str) -> Result<(String, String), String> {
    if asset_name.ends_with(".zip") {
        Ok(("deoxidizer.exe".to_string(), "deox.exe".to_string()))
    } else if asset_name.ends_with(".tar.gz") {
        Ok(("deoxidizer".to_string(), "deox".to_string()))
    } else {
        Err(format!("unknown archive format: {asset_name}"))
    }
}

fn validate_release_proof(
    bytes: &[u8],
    binaries: &BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    let proof: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("release proof is not valid JSON: {error}"))?;
    if proof
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("release proof has an unsupported schema_version".to_string());
    }
    if proof
        .get("target")
        .and_then(serde_json::Value::as_str)
        .is_none()
        || proof
            .get("commit")
            .and_then(serde_json::Value::as_str)
            .is_none()
        || !proof
            .get("native")
            .is_some_and(serde_json::Value::is_object)
    {
        return Err("release proof is missing target, commit, or native metadata".to_string());
    }
    let artifacts = proof
        .get("artifacts")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "release proof is missing its artifacts object".to_string())?;
    if artifacts.len() != binaries.len() {
        return Err("release proof artifacts must name both updater binaries exactly".to_string());
    }
    for (name, digest) in binaries {
        let actual = artifacts
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("release proof is missing artifact hash for {name}"))?;
        if actual.len() != 64
            || !actual
                .chars()
                .all(|character| character.is_ascii_hexdigit())
            || !actual.eq_ignore_ascii_case(digest)
        {
            return Err(format!("release proof artifact hash does not match {name}"));
        }
    }
    Ok(proof)
}

fn extract_update_archive(
    archive_bytes: &[u8],
    asset_name: &str,
    destination: &Path,
) -> Result<(), String> {
    if asset_name.ends_with(".tar.gz") {
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive_bytes));
        extract_tar(decoder, destination)
    } else if asset_name.ends_with(".zip") {
        extract_zip(archive_bytes, destination)
    } else {
        Err(format!("Unknown archive format: {asset_name}"))
    }
}

fn prepare_extracted_binary(path: &Path, asset_name: &str) -> Result<(), String> {
    #[cfg(unix)]
    if asset_name.ends_with(".zip") {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("Failed to chmod extracted binary: {error}"))?;
    }
    let _ = (path, asset_name);
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("Failed to open binary: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Failed to hash binary: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn find_entries_named(dir: &Path, name: &str) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for result in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
    {
        let entry =
            result.map_err(|error| format!("Failed to inspect extracted archive: {error}"))?;
        if entry.file_name().to_string_lossy() == name && entry.file_type().is_file() {
            found.push(entry.path().to_path_buf());
        }
    }
    Ok(found)
}

fn find_unique_binary(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let found = find_entries_named(dir, name)?;
    match found.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(format!("Could not find '{name}' in extracted archive")),
        _ => Err(format!("Archive contains multiple '{name}' binaries")),
    }
}

/// Extract and replace both invokers from a durably staged, authenticated
/// package. The package directory is retained if either replacement fails,
/// so the next invocation can recover before the latest-version early return.
fn install_update(
    archive_bytes: &[u8],
    asset_name: &str,
    manifest: &[u8],
    signature: &[u8],
) -> Result<(), InstallFailure> {
    verify_signed_manifest(manifest, signature).map_err(InstallFailure::Failed)?;
    let checksums = std::str::from_utf8(manifest).map_err(|error| {
        InstallFailure::Failed(format!("Checksum manifest is not UTF-8: {error}"))
    })?;
    if !verify_sha256(archive_bytes, asset_name, checksums) {
        return Err(InstallFailure::Failed(
            "Downloaded archive no longer matches the verified checksum manifest".to_string(),
        ));
    }
    let (archive_preflight, _report) =
        inspect_release_archive(archive_bytes, asset_name).map_err(InstallFailure::Failed)?;
    drop(archive_preflight);
    let current_exe = std::env::current_exe().map_err(|error| {
        InstallFailure::Failed(format!("Could not locate current executable: {error}"))
    })?;
    preflight_update_destinations(&current_exe).map_err(InstallFailure::Failed)?;
    let parent = current_exe.parent().ok_or_else(|| {
        InstallFailure::Failed("Current executable has no parent directory".to_string())
    })?;
    let pending = write_pending_update(parent, archive_bytes, asset_name, manifest, signature)
        .map_err(InstallFailure::Failed)?;
    apply_pending_update(&pending, &current_exe)
}

fn write_pending_update(
    parent: &Path,
    archive_bytes: &[u8],
    asset_name: &str,
    manifest: &[u8],
    signature: &[u8],
) -> Result<PathBuf, String> {
    if !safe_asset_name(asset_name) {
        return Err("update asset name is invalid".to_string());
    }
    let checksums = std::str::from_utf8(manifest)
        .map_err(|error| format!("Checksum manifest is not UTF-8: {error}"))?;
    if !verify_sha256(archive_bytes, asset_name, checksums) {
        return Err("update archive does not match signed checksum manifest".to_string());
    }
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("Could not inspect install directory: {error}"))?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err("install directory is not a real directory".to_string());
    }
    let pending = parent.join(PENDING_UPDATE_DIRECTORY);
    match fs::symlink_metadata(&pending) {
        Ok(_) => return Err("a pending update recovery package already exists".to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not inspect pending update path: {error}")),
    }

    let stage = tempfile::Builder::new()
        .prefix("deoxidizer-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("Could not create update recovery staging directory: {error}"))?;
    write_new_synced(&stage.path().join("archive"), archive_bytes)?;
    write_new_synced(&stage.path().join("manifest"), manifest)?;
    write_new_synced(&stage.path().join("signature.minisig"), signature)?;
    write_new_synced(&stage.path().join("asset-name"), asset_name.as_bytes())?;
    sync_directory(stage.path())?;
    fs::rename(stage.path(), &pending)
        .map_err(|error| format!("Could not publish update recovery package: {error}"))?;
    sync_directory(parent)?;
    Ok(pending)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not sync update directory: {error}"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create staged update file: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write staged update file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not sync staged update file: {error}"))
}

fn safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && (name.ends_with(".zip") || name.ends_with(".tar.gz"))
}

fn read_bounded_regular_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect recovery package file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("recovery package contains a non-regular file".to_string());
    }
    if metadata.len() > max_bytes {
        return Err("recovery package file exceeds its size limit".to_string());
    }
    fs::read(path).map_err(|error| format!("Could not read recovery package file: {error}"))
}

fn apply_pending_update(pending: &Path, current_exe: &Path) -> Result<(), InstallFailure> {
    apply_pending_update_with(
        pending,
        current_exe,
        verify_signed_manifest,
        |source, _destination| {
            self_replace::self_replace(source)
                .map_err(|error| format!("Failed to replace current executable: {error}"))
        },
        |source, destination| {
            replace_file_atomically(source, destination)
                .map_err(|error| format!("Failed to replace sibling executable: {error}"))
        },
    )
}

fn apply_pending_update_with<V, C, S>(
    pending: &Path,
    current_exe: &Path,
    verify_manifest: V,
    replace_current: C,
    replace_sibling: S,
) -> Result<(), InstallFailure>
where
    V: Fn(&[u8], &[u8]) -> Result<(), String>,
    C: FnOnce(&Path, &Path) -> Result<(), String>,
    S: FnMut(&Path, &Path) -> Result<(), String>,
{
    let expected_pending = current_exe
        .parent()
        .ok_or_else(|| {
            InstallFailure::Failed("Current executable has no parent directory".to_string())
        })?
        .join(PENDING_UPDATE_DIRECTORY);
    if pending != expected_pending {
        return Err(InstallFailure::Failed(
            "Pending update path does not match the install directory".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(pending).map_err(|error| {
        InstallFailure::Failed(format!("Could not inspect pending update package: {error}"))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(InstallFailure::Failed(
            "Pending update path is not a real directory".to_string(),
        ));
    }
    let archive = read_bounded_regular_file(&pending.join("archive"), MAX_ASSET_BYTES as u64)
        .map_err(InstallFailure::Failed)?;
    let manifest = read_bounded_regular_file(&pending.join("manifest"), MAX_CHECKSUM_BYTES as u64)
        .map_err(InstallFailure::Failed)?;
    let signature = read_bounded_regular_file(
        &pending.join("signature.minisig"),
        MAX_SIGNATURE_BYTES as u64,
    )
    .map_err(InstallFailure::Failed)?;
    let asset_name_bytes = read_bounded_regular_file(&pending.join("asset-name"), 512)
        .map_err(InstallFailure::Failed)?;
    let asset_name = std::str::from_utf8(&asset_name_bytes)
        .map_err(|error| InstallFailure::Failed(format!("Invalid recovery asset name: {error}")))?;
    if !safe_asset_name(asset_name) {
        return Err(InstallFailure::Failed(
            "Invalid recovery asset name".to_string(),
        ));
    }
    verify_manifest(&manifest, &signature).map_err(InstallFailure::Failed)?;
    let checksum_text = std::str::from_utf8(&manifest).map_err(|error| {
        InstallFailure::Failed(format!("Invalid recovery checksum manifest: {error}"))
    })?;
    if !verify_sha256(&archive, asset_name, checksum_text) {
        return Err(InstallFailure::Failed(
            "Recovery archive checksum verification failed".to_string(),
        ));
    }

    let (extracted, _report) =
        inspect_release_archive(&archive, asset_name).map_err(InstallFailure::Failed)?;
    let result = replace_extracted_pair_with(
        current_exe,
        extracted.path(),
        asset_name,
        replace_current,
        replace_sibling,
    );
    if result.is_ok() {
        // A cleanup failure is safe: the retained signed package is replayed
        // as a repair on the next invocation, then cleanup is retried.
        let _ = fs::remove_dir_all(pending);
    }
    result
}

fn recover_pending_update() -> Result<bool, String> {
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("Could not locate current executable: {error}"))?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| "Current executable has no parent directory".to_string())?;
    let pending = parent.join(PENDING_UPDATE_DIRECTORY);
    match fs::symlink_metadata(&pending) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect pending update package: {error}")),
        Ok(_) => apply_pending_update(&pending, &current_exe)
            .map(|()| true)
            .map_err(|error| error.to_string()),
    }
}

fn replace_extracted_pair_with<C, S>(
    current_exe: &Path,
    extracted_dir: &Path,
    asset_name: &str,
    replace_current: C,
    mut replace_sibling: S,
) -> Result<(), InstallFailure>
where
    C: FnOnce(&Path, &Path) -> Result<(), String>,
    S: FnMut(&Path, &Path) -> Result<(), String>,
{
    let sibling_dest =
        preflight_update_destinations(current_exe).map_err(InstallFailure::Failed)?;
    let (primary_name, alias_name) =
        archive_binary_names_for_asset(asset_name).map_err(InstallFailure::Failed)?;
    let current_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            InstallFailure::Failed("Current executable name is not UTF-8".to_string())
        })?;
    let current_binary_name = if current_name == primary_name || current_name == alias_name {
        current_name
    } else {
        return Err(InstallFailure::Failed(format!(
            "Current executable name is not a supported invoker: {current_name}"
        )));
    };
    let sibling_binary = if current_binary_name == primary_name {
        alias_name.as_str()
    } else {
        primary_name.as_str()
    };
    let current_source =
        find_unique_binary(extracted_dir, current_binary_name).map_err(InstallFailure::Failed)?;
    let sibling_source =
        find_unique_binary(extracted_dir, sibling_binary).map_err(InstallFailure::Failed)?;

    replace_current(&current_source, current_exe).map_err(InstallFailure::Failed)?;
    if let Some(sibling_dest) = sibling_dest {
        // Close the preflight-to-use gap; if the destination changed, the
        // staged package remains for a later safe recovery attempt.
        let latest_sibling = preflight_update_destinations(current_exe)
            .map_err(InstallFailure::MainUpdatedSiblingFailed)?;
        if latest_sibling.as_deref() != Some(sibling_dest.as_path()) {
            return Err(InstallFailure::MainUpdatedSiblingFailed(
                "Sibling destination changed during update".to_string(),
            ));
        }
        replace_sibling(&sibling_source, &sibling_dest)
            .map_err(InstallFailure::MainUpdatedSiblingFailed)?;
    }
    Ok(())
}

fn preflight_update_destinations(current_exe: &Path) -> Result<Option<PathBuf>, String> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| "Current executable has no parent directory".to_string())?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("Could not inspect install directory: {error}"))?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err("Install directory is not a real directory".to_string());
    }
    let current_metadata = fs::symlink_metadata(current_exe)
        .map_err(|error| format!("Could not inspect current executable: {error}"))?;
    if !current_metadata.is_file() || current_metadata.file_type().is_symlink() {
        return Err("Current executable destination is not a regular file".to_string());
    }
    #[cfg(windows)]
    if current_metadata.permissions().readonly() {
        return Err("Current executable destination is read-only".to_string());
    }

    let current_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Current executable name is not UTF-8".to_string())?;
    let sibling_name = sibling_binary_name(current_name)
        .ok_or_else(|| format!("Unsupported current executable name: {current_name}"))?;
    let sibling_dest = parent.join(sibling_name);
    let sibling_metadata = match fs::symlink_metadata(&sibling_dest) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            probe_directory_writable(parent)?;
            return Ok(None);
        }
        Err(error) => return Err(format!("Could not inspect sibling destination: {error}")),
    };
    if sibling_metadata.file_type().is_symlink() {
        let current_real = fs::canonicalize(current_exe)
            .map_err(|error| format!("Could not resolve current executable: {error}"))?;
        let sibling_real = fs::canonicalize(&sibling_dest)
            .map_err(|error| format!("Sibling symlink is dangling or unreadable: {error}"))?;
        if sibling_real == current_real {
            probe_directory_writable(parent)?;
            return Ok(None);
        }
        return Err("Sibling destination is an unexpected symlink".to_string());
    }
    if !sibling_metadata.is_file() {
        return Err("Sibling destination is not a regular file".to_string());
    }
    #[cfg(windows)]
    if sibling_metadata.permissions().readonly() {
        return Err("Sibling destination is read-only".to_string());
    }
    probe_directory_writable(parent)?;
    Ok(Some(sibling_dest))
}

fn probe_directory_writable(parent: &Path) -> Result<(), String> {
    tempfile::Builder::new()
        .prefix(".deoxidizer-update-preflight-")
        .tempfile_in(parent)
        .map_err(|error| format!("Install directory is not writable: {error}"))?;
    Ok(())
}

/// Replace `dest` with the bytes of `src` via a temp file in `dest`'s parent
/// directory followed by an atomic rename. In-place overwrite is avoided: on
/// macOS it can get a previously-executed signed binary killed, and on Linux
/// it fails with ETXTBSY while the file is running. The destination keeps its
/// existing permissions (fallback 0o755 on unix).
fn replace_file_atomically(src: &Path, dest: &Path) -> io::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| io::Error::other("destination has no parent directory"))?;
    let mut temp = tempfile::Builder::new()
        .prefix(".deox-sibling-")
        .tempfile_in(parent)?;
    let mut input = fs::File::open(src)?;
    io::copy(&mut input, temp.as_file_mut())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dest)
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .unwrap_or(0o755);
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(mode))?;
    }
    temp.as_file().sync_all()?;
    temp.persist(dest).map_err(|error| error.error)?;
    Ok(())
}

fn sibling_binary_name(current_name: &str) -> Option<&'static str> {
    match current_name {
        "deoxidizer" => Some("deox"),
        "deox" => Some("deoxidizer"),
        "deoxidizer.exe" => Some("deox.exe"),
        "deox.exe" => Some("deoxidizer.exe"),
        _ => None,
    }
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
    let mut seen_paths = HashSet::new();
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("Failed to read tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("Invalid tar path: {error}"))?
            .to_path_buf();
        if path.to_string_lossy().contains('\\') {
            return Err(format!("archive contains unsafe path {}", path.display()));
        }
        let safe_path = validate_archive_path(&path)?;
        if !seen_paths.insert(safe_path.to_string_lossy().to_lowercase()) {
            return Err(format!(
                "archive contains duplicate path {}",
                path.display()
            ));
        }
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
            // User-facing path stays archive-relative; absolute temp
            // prefixes never leave this module.
            fs::create_dir_all(&output)
                .map_err(|error| format!("Failed to create {}: {error}", safe_path.display()))?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(format!(
                "archive contains unsupported entry {}",
                path.display()
            ));
        }
        let mode = entry.header().mode().map_err(|error| error.to_string())?;
        if mode & 0o6000 != 0 {
            return Err(format!(
                "archive contains setuid/setgid entry {}",
                path.display()
            ));
        }
        // Declared-size precheck before any bytes hit disk; authoritative
        // accounting below uses actual unpacked bytes.
        let declared = entry.header().size().map_err(|error| error.to_string())?;
        let precheck_exceeds = match extracted_bytes.checked_add(declared) {
            None => true,
            Some(total) => total > MAX_EXTRACTED_BYTES,
        };
        if precheck_exceeds {
            return Err("archive exceeds extracted size limit".to_string());
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                let rel = parent
                    .strip_prefix(destination)
                    .unwrap_or(&safe_path)
                    .display()
                    .to_string();
                format!("Failed to create {rel}: {error}")
            })?;
        }
        entry
            .unpack(&output)
            .map_err(|error| format!("Failed to extract {}: {error}", path.display()))?;
        // Accumulate actual bytes written, not just the declared header
        // size, so a lying `size` field cannot bypass the cap.
        let actual = fs::metadata(&output)
            .map_err(|error| format!("Failed to stat {}: {error}", safe_path.display()))?
            .len();
        extracted_bytes = extracted_bytes
            .checked_add(actual)
            .ok_or_else(|| "archive extracted size overflow".to_string())?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err("archive exceeds extracted size limit".to_string());
        }
    }
    Ok(())
}

fn extract_zip(archive_bytes: &[u8], destination: &Path) -> Result<(), String> {
    let cursor = std::io::Cursor::new(archive_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|error| format!("Failed to open zip: {error}"))?;
    let mut extracted_bytes = 0u64;
    let mut seen_paths = HashSet::new();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Failed to read zip entry: {error}"))?;
        let raw_name = entry
            .name()
            .map_err(|error| format!("Invalid zip entry name: {error}"))?
            .into_owned();
        if raw_name.contains('\\') {
            return Err(format!("archive contains unsafe path {raw_name}"));
        }
        let path = Path::new(&raw_name);
        let safe_path = validate_archive_path(path)?;
        if !seen_paths.insert(safe_path.to_string_lossy().to_lowercase()) {
            return Err(format!("archive contains duplicate path {raw_name}"));
        }
        let unix_mode = entry.unix_mode();
        if unix_mode.is_some_and(|mode| mode & 0o170000 == 0o120000) {
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
                .map_err(|error| format!("Failed to create {raw_name}: {error}"))?;
            continue;
        }
        // Mirror `extract_tar`: setuid/setgid entries reject the whole
        // archive fail-closed instead of landing on disk stripped. zip 9's
        // `unix_mode()` already clears these bits, so inspect the raw
        // external attributes (unix mode lives in the high 16 bits).
        if (entry.external_attributes() >> 16) & 0o6000 != 0 {
            return Err(format!("archive contains setuid/setgid entry {raw_name}"));
        }
        // Declared-size precheck; authoritative accounting uses the actual
        // `io::copy` byte count below.
        let declared = entry.size();
        let precheck_exceeds = match extracted_bytes.checked_add(declared) {
            None => true,
            Some(total) => total > MAX_EXTRACTED_BYTES,
        };
        if precheck_exceeds {
            return Err("archive exceeds extracted size limit".to_string());
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                let rel = parent
                    .strip_prefix(destination)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                format!("Failed to create {rel}: {error}")
            })?;
        }
        let mut output_file = fs::File::create(&output)
            .map_err(|error| format!("Failed to create {raw_name}: {error}"))?;
        let written = io::copy(&mut entry, &mut output_file)
            .map_err(|error| format!("Failed to extract {raw_name}: {error}"))?;
        drop(output_file);
        extracted_bytes = extracted_bytes
            .checked_add(written)
            .ok_or_else(|| "archive extracted size overflow".to_string())?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err("archive exceeds extracted size limit".to_string());
        }
        // Honor the entry's unix mode when present (setuid/setgid already
        // rejected above, so `0o777` only carries ordinary permission bits);
        // otherwise leave permissions alone. Only the located binary is made
        // executable later in `install_update` — never every file.
        #[cfg(unix)]
        if let Some(mode) = unix_mode {
            use std::os::unix::fs::PermissionsExt;
            let perm = mode & 0o777;
            // Only apply when the archive actually carries permission bits;
            // a zero mode means "no opinion", not "remove all permissions".
            if perm != 0 {
                fs::set_permissions(&output, fs::Permissions::from_mode(perm))
                    .map_err(|error| format!("Failed to chmod {raw_name}: {error}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build an in-memory stored (uncompressed) zip; always available with
    /// the deflate-only feature set, so tests need no extra features.
    fn build_zip(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            for (name, mode, data) in entries {
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .unix_permissions(*mode);
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    fn extract_to_scratch(zip_bytes: &[u8]) -> (tempfile::TempDir, Result<(), String>) {
        let dir = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let result = extract_zip(zip_bytes, dir.path());
        (dir, result)
    }

    /// Overwrite the first central-directory entry's declared uncompressed
    /// size (little-endian u32 at signature+24) to simulate a lying header.
    fn patch_central_uncompressed_size(zip_bytes: &mut [u8], file_name: &str, new_size: u32) {
        let pos = zip_bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory header");
        let name_len = u16::from_le_bytes([zip_bytes[pos + 28], zip_bytes[pos + 29]]) as usize;
        assert_eq!(
            &zip_bytes[pos + 46..pos + 46 + name_len],
            file_name.as_bytes()
        );
        zip_bytes[pos + 24..pos + 28].copy_from_slice(&new_size.to_le_bytes());
    }

    /// Overwrite the first central-directory entry's external attributes
    /// (little-endian u32 at signature+38). The public writer API masks
    /// modes to `0o777`, so attacker-controlled file-type/setuid bits can
    /// only be simulated by patching the raw header — which is also exactly
    /// what a malicious archive on the wire looks like.
    fn patch_central_external_attributes(zip_bytes: &mut [u8], attrs: u32) {
        let pos = zip_bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory header");
        zip_bytes[pos + 38..pos + 42].copy_from_slice(&attrs.to_le_bytes());
    }

    fn assert_contained(dir: &tempfile::TempDir) {
        for result in walkdir::WalkDir::new(dir.path()).follow_links(false) {
            let entry = result.unwrap();
            assert!(entry.path().starts_with(dir.path()));
            assert!(!entry.file_type().is_symlink());
        }
    }

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
        assert!(is_allowed_download_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/x?sig=y"
        ));
        assert!(!is_allowed_download_url(
            "https://release-assets.githubusercontent.com.evil.example/x"
        ));
        assert!(!is_allowed_download_url(
            "https://github.com/another-owner/deoxidizer/releases/download/v1/a.tar.gz"
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

    #[test]
    fn zip_symlink_entry_rejects_whole_archive() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.add_symlink("link", "target", options).unwrap();
            writer.finish().unwrap();
        }
        let (dir, result) = extract_to_scratch(&buf.into_inner());
        let error = result.expect_err("symlink entry must be rejected");
        assert!(error.contains("link entry"), "unexpected: {error}");
        assert_contained(&dir);
    }

    #[test]
    fn zip_setuid_and_setgid_entries_reject_whole_archive() {
        for mode in [0o104755, 0o102755, 0o106755] {
            let mut zip_bytes = build_zip(&[("tool", 0o644, b"binary")]);
            patch_central_external_attributes(&mut zip_bytes, (mode as u32) << 16);
            let (dir, result) = extract_to_scratch(&zip_bytes);
            let error = result.expect_err("setuid/setgid entry must be rejected");
            assert!(error.contains("setuid/setgid"), "unexpected: {error}");
            assert_contained(&dir);
        }
    }

    #[test]
    fn zip_backslash_traversal_and_absolute_paths_rejected() {
        for name in ["evil\\tool", "..\\tool", "../evil", "/absolute/evil", ".."] {
            let zip_bytes = build_zip(&[(name, 0o644, b"x")]);
            let (dir, result) = extract_to_scratch(&zip_bytes);
            assert!(result.is_err(), "name must be rejected: {name}");
            assert_contained(&dir);
        }
    }

    #[test]
    fn zip_curdir_prefix_extracts_inside_destination() {
        let zip_bytes = build_zip(&[("./deoxidizer", 0o644, b"binary")]);
        let (dir, result) = extract_to_scratch(&zip_bytes);
        result.expect("CurDir-prefixed path must extract");
        assert_eq!(fs::read(dir.path().join("deoxidizer")).unwrap(), b"binary");
        assert_contained(&dir);
    }

    #[test]
    fn zip_valid_entry_extracts_with_content_intact() {
        let zip_bytes = build_zip(&[("deoxidizer", 0o644, b"binary-bytes")]);
        let (dir, result) = extract_to_scratch(&zip_bytes);
        result.expect("valid entry must extract");
        assert_eq!(
            fs::read(dir.path().join("deoxidizer")).unwrap(),
            b"binary-bytes"
        );
        assert_contained(&dir);
    }

    #[test]
    fn zip_lying_declared_size_hits_precheck_before_disk_write() {
        let mut zip_bytes = build_zip(&[("tool", 0o644, b"tiny")]);
        patch_central_uncompressed_size(&mut zip_bytes, "tool", 600_000_000);
        let (dir, result) = extract_to_scratch(&zip_bytes);
        let error = result.expect_err("declared-size bomb must be rejected");
        assert!(
            error.contains("exceeds extracted size limit"),
            "unexpected: {error}"
        );
        assert_contained(&dir);
    }

    #[test]
    fn zip_non_utf8_name_cannot_escape_destination() {
        let mut zip_bytes = build_zip(&[("tool", 0o644, b"tiny")]);
        // Corrupt the central-directory filename to invalid UTF-8; the
        // reader lossy-converts it, and every downstream check still applies.
        let pos = zip_bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory header");
        zip_bytes[pos + 46] = 0xFF;
        let (dir, result) = extract_to_scratch(&zip_bytes);
        // Either outcome is safe as long as nothing escapes the destination.
        let _ = result;
        assert_contained(&dir);
    }

    #[test]
    fn semver_prerelease_orders_before_stable() {
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(is_newer("0.2.0-beta.2", "0.2.0-beta.1"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn sibling_binary_names_match_each_platform() {
        assert_eq!(sibling_binary_name("deoxidizer"), Some("deox"));
        assert_eq!(sibling_binary_name("deox"), Some("deoxidizer"));
        assert_eq!(sibling_binary_name("deoxidizer.exe"), Some("deox.exe"));
        assert_eq!(sibling_binary_name("deox.exe"), Some("deoxidizer.exe"));
        assert_eq!(sibling_binary_name("unexpected"), None);
    }

    // Failure modes for `replace_file_atomically`, written before the helper:
    // - missing source must error and leave the destination byte-identical;
    // - a failed attempt must not leave temp residue beside the destination;
    // - success must replace content, keep the destination's mode, and on
    //   unix swap in a new inode (never overwrite the old file in place).
    #[test]
    fn replace_file_atomically_missing_source_leaves_destination_intact() {
        let dir = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let dest = dir.path().join("deox");
        fs::write(&dest, b"old-bytes").unwrap();
        let result = replace_file_atomically(&dir.path().join("missing"), &dest);
        assert!(result.is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"old-bytes");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["deox".to_string()], "temp residue: {names:?}");
    }

    #[test]
    fn replace_file_atomically_replaces_content_and_keeps_mode() {
        let dir = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let src = dir.path().join("new-deox");
        let dest = dir.path().join("deox");
        fs::write(&src, b"new-binary-bytes").unwrap();
        fs::write(&dest, b"old-bytes").unwrap();
        #[cfg(unix)]
        let old_inode = {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o700)).unwrap();
            fs::metadata(&dest).unwrap().ino()
        };

        replace_file_atomically(&src, &dest).expect("atomic replace must succeed");

        assert_eq!(fs::read(&dest).unwrap(), b"new-binary-bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let meta = fs::metadata(&dest).unwrap();
            assert_ne!(meta.ino(), old_inode, "destination must be a new inode");
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "unexpected residue: {names:?}");
    }

    fn archive_binary_names() -> (&'static str, &'static str) {
        ("deoxidizer.exe", "deox.exe")
    }

    fn proof_json_for(first_name: &str, first: &[u8], second_name: &str, second: &[u8]) -> String {
        let first_hash = hex::encode(Sha256::digest(first));
        let second_hash = hex::encode(Sha256::digest(second));
        format!(
            "{{\"schema_version\":1,\"target\":\"host\",\"commit\":\"0123456789012345678901234567890123456789\",\"artifacts\":{{\"{first_name}\":\"{first_hash}\",\"{second_name}\":\"{second_hash}\"}},\"native\":{{\"kind\":\"local\",\"signature_verified\":true,\"timestamp_verified\":true,\"notarization_status\":\"Accepted\"}}}}"
        )
    }

    #[test]
    fn release_archive_validation_returns_matching_binary_hashes_and_proof() {
        let (deoxidizer, deox) = archive_binary_names();
        let proof = proof_json_for(deoxidizer, b"primary", deox, b"alias");
        let archive = build_zip(&[
            (deoxidizer, 0o755, b"primary"),
            (deox, 0o755, b"alias"),
            (RELEASE_PROOF_NAME, 0o644, proof.as_bytes()),
            ("LICENSE.txt", 0o644, b"GPL-3.0-or-later"),
        ]);

        let report = verify_release_archive(&archive, "deoxidizer-test-windows-x86_64.zip")
            .expect("valid production archive should pass");

        assert_eq!(
            report.binaries[deoxidizer],
            hex::encode(Sha256::digest(b"primary"))
        );
        assert_eq!(report.binaries[deox], hex::encode(Sha256::digest(b"alias")));
        assert_eq!(report.proof_json.unwrap()["schema_version"], 1);
    }

    #[test]
    fn release_archive_validation_requires_unique_dual_binaries() {
        let (deoxidizer, deox) = archive_binary_names();
        let missing = build_zip(&[(deoxidizer, 0o755, b"primary")]);
        assert!(verify_release_archive(&missing, "deoxidizer-test-windows-x86_64.zip").is_err());

        let duplicate = build_zip(&[
            ("first/deoxidizer.exe", 0o755, b"primary-a"),
            ("second/deoxidizer.exe", 0o755, b"primary-b"),
            (deox, 0o755, b"alias"),
        ]);
        assert!(verify_release_archive(&duplicate, "deoxidizer-test-windows-x86_64.zip").is_err());
    }

    #[test]
    fn release_archive_proof_must_match_both_binary_digests() {
        let (deoxidizer, deox) = archive_binary_names();
        let bad_proof = proof_json_for(deoxidizer, b"wrong", deox, b"alias");
        let archive = build_zip(&[
            (deoxidizer, 0o755, b"primary"),
            (deox, 0o755, b"alias"),
            (RELEASE_PROOF_NAME, 0o644, bad_proof.as_bytes()),
        ]);

        let error = verify_release_archive(&archive, "deoxidizer-test-windows-x86_64.zip")
            .expect_err("proof hash must match extracted bytes");
        assert!(error.contains("artifact hash"), "unexpected error: {error}");
    }

    #[test]
    fn release_archive_rejects_duplicate_proof_entries() {
        let (deoxidizer, deox) = archive_binary_names();
        let proof = proof_json_for(deoxidizer, b"primary", deox, b"alias");
        let archive = build_zip(&[
            (deoxidizer, 0o755, b"primary"),
            (deox, 0o755, b"alias"),
            (RELEASE_PROOF_NAME, 0o644, proof.as_bytes()),
            ("nested/DEOX-RELEASE-PROOF.json", 0o644, proof.as_bytes()),
        ]);

        let error = verify_release_archive(&archive, "deoxidizer-test.zip")
            .expect_err("duplicate proof basenames must be rejected");
        assert!(error.contains("proof"), "unexpected error: {error}");
    }

    #[test]
    fn request_timeout_uses_remaining_budget_and_operation_cap() {
        let deadline = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            timeout_for_budget(deadline, Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );

        let short_deadline = Instant::now() + Duration::from_secs(2);
        let short = timeout_for_budget(short_deadline, Duration::from_secs(30)).unwrap();
        assert!(short <= Duration::from_secs(2));
        assert!(short > Duration::ZERO);
        assert_eq!(
            timeout_for_budget(Instant::now(), Duration::from_secs(1)),
            None
        );

        let agent = http_agent(Instant::now() + Duration::from_secs(60))
            .expect("unexpired update deadline should build an agent");
        let config = format!("{agent:?}");
        assert!(
            config.contains("timeout: Some("),
            "overall timeout missing: {config}"
        );
        assert!(
            config.contains("timeout_connect: Some("),
            "connect budget missing: {config}"
        );
        assert!(
            config.contains("timeout_read: Some("),
            "read budget missing: {config}"
        );
        assert!(
            config.contains("timeout_write: Some("),
            "write budget missing: {config}"
        );
    }

    fn make_signed_package_fixture(parent: &Path) -> (PathBuf, String) {
        let (deoxidizer, deox) = archive_binary_names();
        let archive = build_zip(&[
            (deoxidizer, 0o755, b"new-primary"),
            (deox, 0o755, b"new-alias"),
        ]);
        let asset_name = "deoxidizer-v9.9.9-windows-x86_64.zip".to_string();
        let digest = hex::encode(Sha256::digest(&archive));
        let manifest = format!("{digest}  {asset_name}\n");
        let pending = write_pending_update(
            parent,
            &archive,
            &asset_name,
            manifest.as_bytes(),
            b"signature",
        )
        .expect("write pending fixture");
        (pending, asset_name)
    }

    #[test]
    fn pending_update_is_retained_after_sibling_failure_and_same_version_retry_repairs_both() {
        let root = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let (current_name, sibling_name) = archive_binary_names();
        let current = root.path().join(current_name);
        let sibling = root.path().join(sibling_name);
        fs::write(&current, b"old-primary").unwrap();
        fs::write(&sibling, b"old-alias").unwrap();
        let (pending, _) = make_signed_package_fixture(root.path());

        let first = apply_pending_update_with(
            &pending,
            &current,
            |manifest, signature| {
                assert!(std::str::from_utf8(manifest)
                    .unwrap()
                    .contains("deoxidizer-v9.9.9-windows-x86_64.zip"));
                assert_eq!(signature, b"signature");
                Ok(())
            },
            |src, dest| fs::copy(src, dest).map(|_| ()).map_err(|e| e.to_string()),
            |_, _| Err("injected sibling replacement failure".to_string()),
        );
        assert!(matches!(
            first,
            Err(InstallFailure::MainUpdatedSiblingFailed(_))
        ));
        assert_eq!(fs::read(&current).unwrap(), b"new-primary");
        assert_eq!(fs::read(&sibling).unwrap(), b"old-alias");
        assert!(
            pending.exists(),
            "recovery package must survive partial install"
        );

        let second = apply_pending_update_with(
            &pending,
            &sibling,
            |_, _| Ok(()),
            |src, dest| fs::copy(src, dest).map(|_| ()).map_err(|e| e.to_string()),
            |src, dest| fs::copy(src, dest).map(|_| ()).map_err(|e| e.to_string()),
        );
        second.expect("same-version recovery should finish from either invoker");
        assert_eq!(fs::read(&current).unwrap(), b"new-primary");
        assert_eq!(fs::read(&sibling).unwrap(), b"new-alias");
        assert!(
            !pending.exists(),
            "completed recovery package must be removed"
        );
    }

    #[test]
    fn pending_update_signature_failure_leaves_both_live_binaries_untouched() {
        let root = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let (current_name, sibling_name) = archive_binary_names();
        let current = root.path().join(current_name);
        let sibling = root.path().join(sibling_name);
        fs::write(&current, b"old-primary").unwrap();
        fs::write(&sibling, b"old-alias").unwrap();
        let (pending, _) = make_signed_package_fixture(root.path());
        let mut replaced = false;

        let result = apply_pending_update_with(
            &pending,
            &current,
            |_, _| Err("bad signature".to_string()),
            |_, _| {
                replaced = true;
                Ok(())
            },
            |_, _| Ok(()),
        );

        assert!(result.is_err());
        assert!(!replaced);
        assert_eq!(fs::read(&current).unwrap(), b"old-primary");
        assert_eq!(fs::read(&sibling).unwrap(), b"old-alias");
        assert!(
            pending.exists(),
            "unverified recovery package is retained for diagnosis"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_sibling_symlink_is_rejected_before_current_replacement() {
        use std::os::unix::fs::symlink;

        let root = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        let current = root.path().join("deoxidizer");
        let sibling = root.path().join("deox");
        let outside = root.path().join("outside");
        fs::write(&current, b"old-primary").unwrap();
        fs::write(&outside, b"outside-sentinel").unwrap();
        symlink(&outside, &sibling).unwrap();
        let extracted = tempfile::Builder::new()
            .prefix("deoxidizer-test-")
            .tempdir()
            .unwrap();
        fs::write(extracted.path().join("deoxidizer"), b"new-primary").unwrap();
        fs::write(extracted.path().join("deox"), b"new-alias").unwrap();
        let mut called = false;

        let result = replace_extracted_pair_with(
            &current,
            extracted.path(),
            "deoxidizer-test-linux-x86_64.tar.gz",
            |_, _| {
                called = true;
                Ok(())
            },
            |_, _| Ok(()),
        );

        assert!(result.is_err());
        assert!(
            !called,
            "current executable must stay untouched before preflight"
        );
        assert_eq!(fs::read(&current).unwrap(), b"old-primary");
        assert_eq!(fs::read(&outside).unwrap(), b"outside-sentinel");
    }
}
