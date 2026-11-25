use std::path::PathBuf;

const MAX_RELEASE_ASSET_BYTES: u64 = 256 * 1024 * 1024;

fn main() {
    let mut json = false;
    let mut require_proof = false;
    let mut archive_path: Option<PathBuf> = None;

    for arg in std::env::args_os().skip(1) {
        if arg == "--json" {
            json = true;
        } else if arg == "--require-proof" {
            require_proof = true;
        } else if archive_path.is_none() {
            archive_path = Some(PathBuf::from(arg));
        } else {
            usage();
        }
    }

    let Some(archive_path) = archive_path else {
        usage();
    };
    let file_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name.is_empty() {
        fail("archive path must have a UTF-8 file name");
    }
    let metadata = std::fs::symlink_metadata(&archive_path)
        .unwrap_or_else(|error| fail(&format!("could not inspect archive: {error}")));
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        fail("archive path must name a regular non-symlink file");
    }
    if metadata.len() > MAX_RELEASE_ASSET_BYTES {
        fail("archive exceeds 256 MiB limit");
    }
    let archive = std::fs::read(&archive_path)
        .unwrap_or_else(|error| fail(&format!("could not read archive: {error}")));
    let report = deoxidizer_lib::updater::verify_release_archive(&archive, file_name)
        .unwrap_or_else(|error| fail(&error));
    if require_proof && report.proof_json.is_none() {
        fail("archive does not contain DEOX-RELEASE-PROOF.json");
    }

    if json {
        let value = serde_json::json!({
            "binaries": report.binaries,
            "proof": report.proof_json,
        });
        println!(
            "{}",
            serde_json::to_string(&value).expect("JSON value serializes")
        );
    } else {
        println!("release archive is valid: {file_name}");
    }
}

fn usage() -> ! {
    eprintln!("Usage: verify_release_archive [--json] [--require-proof] <archive-path>");
    std::process::exit(2)
}

fn fail(message: &str) -> ! {
    eprintln!("release archive verification failed: {message}");
    std::process::exit(1)
}
