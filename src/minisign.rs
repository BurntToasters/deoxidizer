//! Fail-closed verification for the repository's pinned Minisign key.

use minisign_verify::{PublicKey, Signature};

const PINNED_PUBLIC_KEY: &str = include_str!("../release-signing-key.pub");
const PINNED_PUBLIC_KEY_COMMENT: &str = "untrusted comment: minisign public key: 083B254A48C428E3";
const PINNED_PUBLIC_KEY_BASE64: &str = "RWTjKMRISiU7CDbPKID/yjE1xQvWSbhYFjDF3xgaJCAqOiO7IPffxndk";
const MAX_SIGNATURE_BYTES: usize = 64 * 1024;
const MAX_PUBLIC_KEY_BYTES: usize = 1024;

/// Verify standard raw Minisign detached-signature bytes with the one
/// repository-pinned key. Legacy Ed signatures are explicitly disabled.
pub(crate) fn verify_detached(data: &[u8], signature_bytes: &[u8]) -> Result<(), String> {
    let public_key = parse_pinned_public_key()?;
    let signature = parse_signature(signature_bytes)?;
    public_key
        .verify(data, &signature, false)
        .map_err(|error| format!("Minisign verification failed: {error}"))
}

fn parse_pinned_public_key() -> Result<PublicKey, String> {
    let lines = strict_ascii_lines(PINNED_PUBLIC_KEY.as_bytes(), 2, MAX_PUBLIC_KEY_BYTES)
        .map_err(|error| format!("Pinned Minisign public key is invalid: {error}"))?;
    if lines[0] != PINNED_PUBLIC_KEY_COMMENT || lines[1] != PINNED_PUBLIC_KEY_BASE64 {
        return Err(
            "Pinned Minisign public key does not match the authorized serialization".into(),
        );
    }
    parse_public_key(PINNED_PUBLIC_KEY.as_bytes())
}

fn parse_public_key(public_key_bytes: &[u8]) -> Result<PublicKey, String> {
    let lines = strict_ascii_lines(public_key_bytes, 2, MAX_PUBLIC_KEY_BYTES)?;
    if !lines[0].starts_with("untrusted comment: minisign public key") {
        return Err("Minisign public key comment is invalid".into());
    }
    if lines[1].is_empty() || !is_base64_line(&lines[1]) {
        return Err("Minisign public key body is invalid".into());
    }
    let normalized = normalize_line_endings(public_key_bytes)?;
    PublicKey::decode(&normalized)
        .map_err(|error| format!("Minisign public key parse failed: {error}"))
}

fn parse_signature(signature_bytes: &[u8]) -> Result<Signature, String> {
    let lines = strict_ascii_lines(signature_bytes, 4, MAX_SIGNATURE_BYTES)?;
    if !lines[0].starts_with("untrusted comment: ") || lines[0].len() <= 18 {
        return Err("Minisign signature comment is invalid".into());
    }
    if !is_base64_line(&lines[1]) {
        return Err("Minisign signature body is invalid".into());
    }
    if !lines[2].starts_with("trusted comment: ") || lines[2].len() <= 17 {
        return Err("Minisign trusted comment is invalid".into());
    }
    if !is_base64_line(&lines[3]) {
        return Err("Minisign global signature is invalid".into());
    }
    let normalized = normalize_line_endings(signature_bytes)?;
    Signature::decode(&normalized)
        .map_err(|error| format!("Minisign signature parse failed: {error}"))
}

fn strict_ascii_lines(
    bytes: &[u8],
    expected_lines: usize,
    max_bytes: usize,
) -> Result<Vec<String>, String> {
    if bytes.is_empty() || bytes.len() > max_bytes || !bytes.is_ascii() {
        return Err("serialized data has invalid size or encoding".into());
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "serialized data is not UTF-8".to_string())?;
    let normalized = normalize_line_endings(text.as_bytes())?;
    let body = normalized.strip_suffix('\n').unwrap_or(&normalized);
    let lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
    if lines.len() != expected_lines || lines.iter().any(|line| line.is_empty()) {
        return Err(format!("expected exactly {expected_lines} nonempty lines"));
    }
    Ok(lines)
}

fn normalize_line_endings(bytes: &[u8]) -> Result<String, String> {
    if !bytes.is_ascii() {
        return Err("serialized data must be ASCII".into());
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "serialized data is not UTF-8".to_string())?;
    let normalized = text.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err("serialized data contains a bare carriage return".into());
    }
    Ok(normalized)
}

fn is_base64_line(line: &str) -> bool {
    !line.is_empty()
        && line
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

#[cfg(test)]
fn verify_signature_with_key(
    public_key_bytes: &[u8],
    data: &[u8],
    signature_bytes: &[u8],
) -> Result<(), String> {
    let public_key = parse_public_key(public_key_bytes)?;
    let signature = parse_signature(signature_bytes)?;
    public_key
        .verify(data, &signature, false)
        .map_err(|error| format!("Minisign verification failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Public test key and signatures are upstream known vectors from
    // minisign-verify 0.3.0. The signing key is not retained or accepted by
    // the production entrypoint.
    const TEST_PUBLIC_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\n\
RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const TEST_NONLEGACY_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
trusted comment: timestamp:1556193335\tfile:test\n\
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n";
    const TEST_LEGACY_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\n\
trusted comment: timestamp:1555779966\tfile:test\n\
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n";

    #[test]
    fn upstream_nonlegacy_minisign_vector_verifies_and_tampering_fails() {
        verify_signature_with_key(
            TEST_PUBLIC_KEY.as_bytes(),
            b"test",
            TEST_NONLEGACY_SIGNATURE.as_bytes(),
        )
        .expect("upstream non-legacy signature vector should verify");

        assert!(verify_signature_with_key(
            TEST_PUBLIC_KEY.as_bytes(),
            b"Test",
            TEST_NONLEGACY_SIGNATURE.as_bytes(),
        )
        .is_err());
    }

    #[test]
    fn legacy_minisign_signature_is_rejected() {
        let error = verify_signature_with_key(
            TEST_PUBLIC_KEY.as_bytes(),
            b"test",
            TEST_LEGACY_SIGNATURE.as_bytes(),
        )
        .expect_err("legacy Ed signatures must not be accepted");
        assert!(error.contains("legacy") || error.contains("algorithm"));
    }

    #[test]
    fn strict_parsers_reject_extra_lines_and_openpgp_armor() {
        let extra_signature = format!("{TEST_NONLEGACY_SIGNATURE}extra\n");
        assert!(parse_signature(extra_signature.as_bytes()).is_err());
        assert!(parse_signature(b"-----BEGIN PGP SIGNATURE-----\n").is_err());

        let extra_key = format!("{TEST_PUBLIC_KEY}unexpected\n");
        assert!(parse_public_key(extra_key.as_bytes()).is_err());
        assert!(parse_public_key(b"not a minisign public key\n").is_err());
    }

    #[test]
    fn production_key_is_exact_pinned_serialization_and_rejects_test_key() {
        assert_eq!(
            PINNED_PUBLIC_KEY,
            include_str!("../release-signing-key.pub")
        );
        assert!(parse_pinned_public_key().is_ok());
        assert!(verify_detached(b"test", TEST_NONLEGACY_SIGNATURE.as_bytes()).is_err());
    }
}
