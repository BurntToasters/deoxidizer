// Release-time cross-check: verify a SHA256SUMS manifest and its detached
// signature with the same in-process verifier that `deox --update` uses.
// Invoked by scripts/verify-release.cjs; not a user-facing tool.
//
// Usage: cargo run --locked --example verify_release_manifest -- <manifest> <manifest>.asc

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: verify_release_manifest <manifest-path> <signature-path>");
        return ExitCode::from(1);
    }
    let manifest_path = &args[1];
    let signature_path = &args[2];

    let manifest = match std::fs::read(manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: cannot read manifest {manifest_path}: {error}");
            return ExitCode::from(1);
        }
    };
    let signature = match std::fs::read(signature_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: cannot read signature {signature_path}: {error}");
            return ExitCode::from(1);
        }
    };

    match deoxidizer_lib::updater::verify_release_manifest(&manifest, &signature) {
        Ok(()) => {
            println!("ok: {manifest_path}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {manifest_path}: {error}");
            ExitCode::from(1)
        }
    }
}
