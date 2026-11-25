// Release-time cross-check for a raw Minisign signature over arbitrary bytes.
// This uses the same pinned key and strict parser as `deox --update`.
//
// Usage: cargo run --locked --example verify_release_signature -- <payload> <payload>.minisig

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: verify_release_signature <payload-path> <signature-path>");
        return ExitCode::from(1);
    }
    let payload_path = &args[1];
    let signature_path = &args[2];

    let payload = match std::fs::read(payload_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: cannot read payload {payload_path}: {error}");
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

    match deoxidizer_lib::updater::verify_release_signature(&payload, &signature) {
        Ok(()) => {
            println!("ok: {payload_path}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {payload_path}: {error}");
            ExitCode::from(1)
        }
    }
}
