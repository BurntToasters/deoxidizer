use colored::Colorize;
use semver::Version;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const REPO_OWNER: &str = "BurntToasters";
const REPO_NAME: &str = "deoxidizer";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_RELEASE_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: usize = 4 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 1024 * 1024;
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REDIRECT_HOPS: usize = 3;
/// Overall network deadline for the update flow (fetch + downloads).
///
/// A single `Instant` deadline created at the start of `run_update` is
/// threaded through `fetch_latest_release`, `download_bytes`, and
/// `read_limited`; every read chunk checks `Instant::now()` against it and
/// aborts on exceed. Single-attempt, no-retry: timeouts fail closed.
const UPDATE_DEADLINE: Duration = Duration::from_secs(150);

/// Expected fingerprint of the pinned release signing key.
///
/// Pinned to `release-signing-key.asc` in the repository root (same key whose
/// fingerprint is enforced by `install.sh`). Signature verification must show
/// a `VALIDSIG` from this fingerprint; there is no rotation mechanism.
const EXPECTED_FPR: &str = "CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F";

/// Install outcome that distinguishes a clean failure from a partial
/// update where the running binary was already replaced but the sibling
/// alias sync failed (version skew). The main binary is new in the latter
/// case, so the caller must say so explicitly instead of reporting a
/// generic failure.
enum InstallFailure {
    Failed(String),
    MainUpdatedSiblingFailed(String),
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

/// Strip the secure tempdir prefix from user-facing errors so absolute
/// `/tmp/deoxidizer-update-*` paths never leak to the terminal. The full
/// path is kept for debug builds only via eprintln below.
fn scrub_temp_dir(message: String, temp_dir: &Path) -> String {
    let prefix = temp_dir.display().to_string();
    let scrubbed = message.replace(&prefix, "<update-tmp>");
    #[cfg(debug_assertions)]
    eprintln!("[debug] update detail: {message}");
    scrubbed
}

/// Run the self-update check and update flow.
///
/// Order is load-bearing: manifest -> asset -> hash -> install. The small
/// checksum manifest and its detached signature are fetched and verified
/// first so a tampered manifest aborts before the large asset download;
/// the asset is hashed against the verified manifest before install.
pub fn run_update() {
    println!();
    println!("  {} Checking for updates...", "🔄".bold());

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
        .and_then(|url| url.rsplit('/').next().map(|name| format!("{name}.asc")));
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
    match install_update(&asset_bytes, &asset_name) {
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

    let response = match request_with_redirect_validation(&url, Some("application/vnd.github+json"))
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
    let response = match request_with_redirect_validation(url, None) {
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
/// `http_agent()` sets `redirects(0)` so ureq never follows a cross-host
/// redirect on its own. Each `Location` target must pass
/// `is_allowed_download_url` before it is followed (relative targets fail
/// closed), and the final `response.get_url()` is asserted against the same
/// allowlist. At most `MAX_REDIRECT_HOPS` redirects are followed; anything
/// else aborts. Signature plus SHA256 verification remains the primary
/// authenticity check.
fn request_with_redirect_validation(
    url: &str,
    accept: Option<&str>,
) -> Result<Result<ureq::Response, ureq::Error>, String> {
    if !is_allowed_download_url(url) {
        return Err("download URL is not HTTPS GitHub-hosted content".to_string());
    }
    let mut current_url = url.to_string();
    for _ in 0..=MAX_REDIRECT_HOPS {
        if !is_allowed_download_url(&current_url) {
            return Err("redirect target is not HTTPS GitHub-hosted content".to_string());
        }
        let mut request = http_agent()
            .get(&current_url)
            .set("User-Agent", &format!("deoxidizer/{}", CURRENT_VERSION));
        if let Some(accept) = accept {
            request = request.set("Accept", accept);
        }
        let response = match request.call() {
            Ok(resp) => resp,
            Err(e) => return Ok(Err(e)),
        };
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

fn http_agent() -> ureq::Agent {
    // Direct HTTPS only by design: no proxy env is honored, so a poisoned
    // `HTTP_PROXY`/`HTTPS_PROXY` cannot reroute the updater. TLS is
    // verified by ureq's defaults; authenticity still rests on the detached
    // signature plus SHA256 (`verify_signed_manifest` + `verify_sha256`).
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .redirects(0)
        .build()
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
            Err(e) => return Err(format!("Read failed: {e}")),
        }
    }
    Ok(bytes)
}

/// Allowlist for updater download URLs.
///
/// The `objects.githubusercontent.com/` prefix is intentionally open:
/// release asset bytes are served from per-file hashed object URLs under
/// that host, so the prefix alone cannot authenticate content. Authenticity
/// comes from the detached signature plus SHA256 verification
/// (`verify_signed_manifest` + `verify_sha256`); redirect targets under this
/// prefix are additionally re-validated hop-by-hop by
/// `request_with_redirect_validation`.
fn is_allowed_download_url(url: &str) -> bool {
    url.starts_with("https://api.github.com/repos/BurntToasters/deoxidizer/")
        || url.starts_with("https://github.com/BurntToasters/deoxidizer/releases/download/")
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

/// Verify checksum manifest using the repository's pinned release key.
///
/// Fail-closed: missing `gpg` aborts the update. An isolated `--homedir`
/// under the secure tempdir keeps the verification off the user's keyring.
fn verify_signed_manifest(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("deoxidizer-update-")
        .tempdir()
        .map_err(|error| format!("cannot create signature workspace: {error}"))?;
    let manifest_path = temp_dir.path().join("SHA256SUMS.txt");
    let signature_path = temp_dir.path().join("SHA256SUMS.txt.asc");
    let key_path = temp_dir.path().join("release-key.asc");
    let keyring_path = temp_dir.path().join("release-keyring.gpg");
    let homedir = temp_dir.path().join("gnupg");
    fs::create_dir_all(&homedir).map_err(|error| format!("cannot create gpg homedir: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&homedir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure gpg homedir: {error}"))?;
    }
    fs::write(&manifest_path, manifest)
        .map_err(|error| format!("cannot write manifest: {error}"))?;
    fs::write(&signature_path, signature)
        .map_err(|error| format!("cannot write signature: {error}"))?;
    fs::write(&key_path, include_bytes!("../release-signing-key.asc"))
        .map_err(|error| format!("cannot write pinned key: {error}"))?;

    let homedir_str = homedir.to_string_lossy().to_string();
    let dearmor = Command::new("gpg")
        .args(["--batch", "--yes", "--no-tty", "--homedir"])
        .arg(&homedir_str)
        .args(["--dearmor", "--output"])
        .arg(&keyring_path)
        .arg(&key_path)
        .output()
        .map_err(|error| format!("gpg unavailable: {error}"))?;
    if !dearmor.status.success() {
        return Err("cannot load pinned release key".to_string());
    }
    let verify = Command::new("gpg")
        .args(["--batch", "--no-tty", "--no-options", "--homedir"])
        .arg(&homedir_str)
        .args(["--no-default-keyring", "--keyring"])
        .arg(&keyring_path)
        .args([
            "--trust-model",
            "direct",
            "--weak-digest",
            "sha1",
            "--status-fd",
            "1",
            "--verify",
        ])
        .arg(&signature_path)
        .arg(&manifest_path)
        .output()
        .map_err(|error| format!("gpg unavailable: {error}"))?;
    if !verify.status.success() {
        let detail = String::from_utf8_lossy(&verify.stderr).trim().to_string();
        if detail.is_empty() {
            return Err("signature does not match pinned release key".to_string());
        }
        return Err(format!(
            "signature does not match pinned release key: {detail}"
        ));
    }
    let status = String::from_utf8_lossy(&verify.stdout);
    // Strict VALIDSIG: the status line must be exactly
    // `[GNUPG:] VALIDSIG <EXPECTED_FPR> ...`. Any BADSIG/ERRSIG/EXPSIG/
    // EXPKEYSIG/REVKEYSIG line rejects, even alongside a VALIDSIG.
    let mut valid = false;
    for line in status.lines() {
        let payload = line.strip_prefix("[GNUPG:] ").unwrap_or(line).trim();
        if payload.starts_with("BADSIG ")
            || payload.starts_with("ERRSIG ")
            || payload.starts_with("EXPSIG ")
            || payload.starts_with("EXPKEYSIG ")
            || payload.starts_with("REVKEYSIG ")
        {
            return Err("signature shows a bad/expired/revoked key".to_string());
        }
        if let Some(rest) = payload.strip_prefix("VALIDSIG ") {
            let fpr = rest.split_whitespace().next().unwrap_or("");
            if fpr.eq_ignore_ascii_case(EXPECTED_FPR) {
                valid = true;
            }
        }
    }
    if !valid {
        let detail = String::from_utf8_lossy(&verify.stderr).trim().to_string();
        if detail.is_empty() {
            return Err("signature is not from the pinned release key".to_string());
        }
        return Err(format!(
            "signature is not from the pinned release key: {detail}"
        ));
    }
    Ok(())
}

/// Extract the update and replace the current binary.
///
/// Note: the secure tempdir is removed when `temp_dir` drops, but with
/// `panic=abort` a mid-install panic cannot run destructors, so residue
/// under `/tmp/deoxidizer-update-*` may remain by design (fail-closed,
/// never half-installed into the live binary path).
///
/// Note: no macOS quarantine (`com.apple.quarantine`) is applied by design;
/// trust comes from the detached signature plus SHA256 (`EXPECTED_FPR`),
/// not from Gatekeeper quarantine bits.
fn install_update(archive_bytes: &[u8], asset_name: &str) -> Result<(), InstallFailure> {
    let temp_dir = tempfile::Builder::new()
        .prefix("deoxidizer-update-")
        .tempdir()
        .map_err(|e| InstallFailure::Failed(format!("Failed to create secure temp dir: {e}")))?;

    let map_temp = |e: String| scrub_temp_dir(e, temp_dir.path());

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
        extract_tar(decoder, temp_dir.path()).map_err(|e| InstallFailure::Failed(map_temp(e)))?;
    } else if asset_name.ends_with(".zip") {
        extract_zip(archive_bytes, temp_dir.path())
            .map_err(|e| InstallFailure::Failed(map_temp(e)))?;
    } else {
        return Err(InstallFailure::Failed(format!(
            "Unknown archive format: {asset_name}"
        )));
    }

    // Find the binary in the extracted files
    let new_binary = find_binary_in_dir(temp_dir.path(), binary_name).ok_or_else(|| {
        InstallFailure::Failed(format!(
            "Could not find '{binary_name}' in extracted archive"
        ))
    })?;

    // Zip archives do not reliably carry unix execute bits (only honored
    // when `unix_mode` is present), so ensure the located binary alone is
    // executable. Tar preserves modes via `extract_tar` and needs no chmod.
    #[cfg(unix)]
    if asset_name.ends_with(".zip") {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755)).map_err(|e| {
            InstallFailure::Failed(map_temp(format!("Failed to chmod binary: {e}")))
        })?;
    }

    // Replace the current running binary first so a sibling-sync failure
    // cannot leave the running binary behind the sibling (version skew).
    self_replace::self_replace(&new_binary)
        .map_err(|e| InstallFailure::Failed(map_temp(format!("Failed to replace binary: {e}"))))?;

    // Keep separately-installed sibling binary in sync. Symlink aliases already
    // resolve to the replaced file and must not be overwritten through a link.
    // Any failure past the `self_replace` above means the main binary is
    // already new, so report MainUpdatedSiblingFailed (caller exits 1 with
    // an explicit "main updated" note) instead of a generic failure.
    if cfg!(target_os = "windows") || cfg!(unix) {
        if let Some(current_exe) = current_exe {
            if let Some(parent) = current_exe.parent() {
                if let Some(current_name) = current_exe.file_name().and_then(|name| name.to_str()) {
                    if let Some(sibling_name) = sibling_binary_name(current_name) {
                        let sibling_dest = parent.join(sibling_name);
                        if fs::symlink_metadata(&sibling_dest)
                            .map(|metadata| {
                                !metadata.file_type().is_symlink() && metadata.is_file()
                            })
                            .unwrap_or(false)
                        {
                            let new_sibling = find_binary_in_dir(temp_dir.path(), sibling_name)
                                .ok_or_else(|| {
                                    InstallFailure::MainUpdatedSiblingFailed(format!(
                                        "Could not find '{sibling_name}' in update"
                                    ))
                                })?;
                            #[cfg(unix)]
                            if asset_name.ends_with(".zip") {
                                use std::os::unix::fs::PermissionsExt;
                                fs::set_permissions(
                                    &new_sibling,
                                    fs::Permissions::from_mode(0o755),
                                )
                                .map_err(|e| {
                                    InstallFailure::MainUpdatedSiblingFailed(map_temp(format!(
                                        "Failed to chmod {sibling_name}: {e}"
                                    )))
                                })?;
                            }
                            // Re-check immediately before copy to close the
                            // check-to-use gap.
                            if fs::symlink_metadata(&sibling_dest)
                                .map(|metadata| {
                                    !metadata.file_type().is_symlink() && metadata.is_file()
                                })
                                .unwrap_or(false)
                            {
                                fs::copy(&new_sibling, &sibling_dest).map_err(|e| {
                                    InstallFailure::MainUpdatedSiblingFailed(map_temp(format!(
                                        "Failed to replace {sibling_name}: {e}"
                                    )))
                                })?;
                            }
                        }
                    }
                }
            }
        }
    }

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
            // User-facing path stays archive-relative; absolute temp
            // prefix never leaves this module (see `scrub_temp_dir`).
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
        // Honor the entry's unix mode when present (stripping setuid/setgid);
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
}
