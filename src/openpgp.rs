//! Minimal, fail-closed verifier for detached OpenPGP v4 signatures made by
//! an Ed25519 key (`gpg --detach-sign --armor` with the release key).
//!
//! This exists so `deox --update` can authenticate release checksum
//! manifests without an external `gpg` binary. It deliberately supports one
//! narrow profile and rejects everything else:
//! - armored detached signature containing exactly one v4 signature packet,
//! - signature type 0x00 (binary document),
//! - public-key algorithm EdDSA (22, Ed25519 curve) or Ed25519 (27),
//! - hash algorithm SHA-256 or SHA-512,
//! - issued by the *primary* key of one of the pinned public keys,
//! - created inside the pinned key's validity window and not expired.
//!
//! Format references: RFC 4880 §4–§5.2 and RFC 9580 §5.2 (v4 signatures).

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256, Sha512};

/// Ed25519 curve OID used by EdDSA (algorithm 22) keys.
const ED25519_OID: [u8; 9] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];
const ALGO_EDDSA_LEGACY: u8 = 22;
const ALGO_ED25519: u8 = 27;
const HASH_SHA256: u8 = 8;
const HASH_SHA512: u8 = 10;
const TAG_SIGNATURE: u8 = 2;
const TAG_PUBLIC_KEY: u8 = 6;
const SUBPACKET_CREATION_TIME: u8 = 2;
const SUBPACKET_EXPIRATION_TIME: u8 = 3;
const SUBPACKET_ISSUER_KEY_ID: u8 = 16;
const SUBPACKET_ISSUER_FINGERPRINT: u8 = 33;

/// A pinned release key: its expected v4 fingerprint, armored public key,
/// and the last Unix time at which it may have created a valid signature.
pub struct PinnedKey {
    pub fingerprint: &'static str,
    pub armored: &'static str,
    pub valid_until: u64,
}

/// Verify `signature_armored` over `data` against the pinned keys.
/// `now` is the current Unix time (used for signature expiration).
pub fn verify_detached(
    data: &[u8],
    signature_armored: &str,
    keys: &[PinnedKey],
    now: u64,
) -> Result<(), String> {
    let signature_bytes = dearmor(signature_armored, "PGP SIGNATURE")?;
    let packets = parse_packets(&signature_bytes)?;
    let [(tag, body)] = packets.as_slice() else {
        return Err("signature must contain exactly one packet".to_string());
    };
    if *tag != TAG_SIGNATURE {
        return Err("armored block is not a signature packet".to_string());
    }
    let signature = parse_signature(body)?;

    for pinned in keys {
        let fingerprint = parse_fingerprint(pinned.fingerprint)?;
        if !signature.issuer_matches(&fingerprint) {
            continue;
        }
        let key = parse_primary_key(pinned.armored)?;
        if signature.created < key.created {
            return Err("signature predates the signing key".to_string());
        }
        if signature.created > pinned.valid_until {
            return Err("signature was created after the pinned key's validity window".to_string());
        }
        if let Some(expires) = signature.expires_after {
            if expires > 0 && now > signature.created.saturating_add(expires) {
                return Err("signature has expired".to_string());
            }
        }
        if signature.algorithm != key.algorithm {
            return Err("signature algorithm does not match the pinned key".to_string());
        }
        let digest = signature.digest(data);
        if digest[..2] != signature.hash_prefix {
            return Err("signature does not match the data".to_string());
        }
        let verifying = VerifyingKey::from_bytes(&key.public)
            .map_err(|_| "pinned key is not a valid Ed25519 key".to_string())?;
        return verifying
            .verify_strict(&digest, &Signature::from_bytes(&signature.value))
            .map_err(|_| "signature does not match the data".to_string());
    }
    Err("signature was not issued by a pinned release key".to_string())
}

struct ParsedSignature {
    algorithm: u8,
    hash: u8,
    hashed_prefix: Vec<u8>,
    hash_prefix: [u8; 2],
    created: u64,
    expires_after: Option<u64>,
    issuer_fingerprint: Option<[u8; 20]>,
    issuer_key_id: Option<[u8; 8]>,
    value: [u8; 64],
}

impl ParsedSignature {
    fn issuer_matches(&self, fingerprint: &[u8; 20]) -> bool {
        match (self.issuer_fingerprint, self.issuer_key_id) {
            (Some(fpr), _) => &fpr == fingerprint,
            (None, Some(id)) => id[..] == fingerprint[12..],
            (None, None) => false,
        }
    }

    /// Hash of `data` plus the v4 signature trailer (RFC 4880 §5.2.4).
    fn digest(&self, data: &[u8]) -> Vec<u8> {
        let mut trailer = self.hashed_prefix.clone();
        trailer.extend_from_slice(&[0x04, 0xFF]);
        trailer.extend_from_slice(&(self.hashed_prefix.len() as u32).to_be_bytes());
        match self.hash {
            HASH_SHA256 => {
                let mut hasher = Sha256::new();
                hasher.update(data);
                hasher.update(&trailer);
                hasher.finalize().to_vec()
            }
            _ => {
                let mut hasher = Sha512::new();
                hasher.update(data);
                hasher.update(&trailer);
                hasher.finalize().to_vec()
            }
        }
    }
}

struct PrimaryKey {
    algorithm: u8,
    created: u64,
    public: [u8; 32],
}

fn parse_fingerprint(hex_fpr: &str) -> Result<[u8; 20], String> {
    let bytes = hex::decode(hex_fpr).map_err(|_| "invalid pinned fingerprint".to_string())?;
    bytes
        .try_into()
        .map_err(|_| "pinned fingerprint must be 20 bytes".to_string())
}

/// Byte cursor that fails closed on every out-of-bounds read.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if count > self.remaining() {
            return Err("truncated OpenPGP data".to_string());
        }
        let slice = &self.bytes[self.pos..self.pos + count];
        self.pos += count;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// Multiprecision integer: 2-byte bit count, then big-endian bytes.
    fn mpi(&mut self) -> Result<&'a [u8], String> {
        let bits = usize::from(self.u16()?);
        self.take(bits.div_ceil(8))
    }
}

fn parse_packets(bytes: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut reader = Reader::new(bytes);
    let mut packets = Vec::new();
    while reader.remaining() > 0 {
        let header = reader.u8()?;
        if header & 0x80 == 0 {
            return Err("invalid OpenPGP packet header".to_string());
        }
        let (tag, len) = if header & 0x40 != 0 {
            let tag = header & 0x3F;
            let first = usize::from(reader.u8()?);
            let len = match first {
                0..=191 => first,
                192..=223 => ((first - 192) << 8) + usize::from(reader.u8()?) + 192,
                255 => reader.u32()? as usize,
                _ => return Err("partial-length OpenPGP packets are not supported".to_string()),
            };
            (tag, len)
        } else {
            let tag = (header >> 2) & 0x0F;
            let len = match header & 0x03 {
                0 => usize::from(reader.u8()?),
                1 => usize::from(reader.u16()?),
                2 => reader.u32()? as usize,
                _ => {
                    return Err("indeterminate-length OpenPGP packets are not supported".to_string())
                }
            };
            (tag, len)
        };
        packets.push((tag, reader.take(len)?));
    }
    Ok(packets)
}

fn parse_signature(body: &[u8]) -> Result<ParsedSignature, String> {
    let mut reader = Reader::new(body);
    if reader.u8()? != 4 {
        return Err("only version 4 signatures are supported".to_string());
    }
    let sig_type = reader.u8()?;
    if sig_type != 0x00 {
        return Err("only binary-document signatures (type 0x00) are accepted".to_string());
    }
    let algorithm = reader.u8()?;
    if algorithm != ALGO_EDDSA_LEGACY && algorithm != ALGO_ED25519 {
        return Err("only Ed25519 signatures are accepted".to_string());
    }
    let hash = reader.u8()?;
    if hash != HASH_SHA256 && hash != HASH_SHA512 {
        return Err("only SHA-256/SHA-512 signatures are accepted".to_string());
    }
    let hashed_len = usize::from(reader.u16()?);
    let hashed = reader.take(hashed_len)?;
    let hashed_prefix = body[..6 + hashed_len].to_vec();
    let unhashed_len = usize::from(reader.u16()?);
    let unhashed = reader.take(unhashed_len)?;
    let prefix = reader.take(2)?;
    let hash_prefix = [prefix[0], prefix[1]];

    let mut value = [0u8; 64];
    if algorithm == ALGO_EDDSA_LEGACY {
        for half in 0..2 {
            let part = reader.mpi()?;
            if part.len() > 32 {
                return Err("malformed EdDSA signature value".to_string());
            }
            let start = half * 32 + (32 - part.len());
            value[start..half * 32 + 32].copy_from_slice(part);
        }
    } else {
        value.copy_from_slice(reader.take(64)?);
    }
    if reader.remaining() != 0 {
        return Err("trailing data after signature".to_string());
    }

    let mut signature = ParsedSignature {
        algorithm,
        hash,
        hashed_prefix,
        hash_prefix,
        created: 0,
        expires_after: None,
        issuer_fingerprint: None,
        issuer_key_id: None,
        value,
    };
    let mut created = None;
    parse_subpackets(hashed, true, &mut signature, &mut created)?;
    parse_subpackets(unhashed, false, &mut signature, &mut created)?;
    signature.created = created.ok_or("signature has no creation time".to_string())?;
    Ok(signature)
}

fn parse_subpackets(
    area: &[u8],
    hashed: bool,
    signature: &mut ParsedSignature,
    created: &mut Option<u64>,
) -> Result<(), String> {
    let mut reader = Reader::new(area);
    while reader.remaining() > 0 {
        let first = usize::from(reader.u8()?);
        let len = match first {
            0..=191 => first,
            192..=254 => ((first - 192) << 8) + usize::from(reader.u8()?) + 192,
            _ => reader.u32()? as usize,
        };
        if len == 0 {
            return Err("empty signature subpacket".to_string());
        }
        let data = reader.take(len)?;
        let critical = data[0] & 0x80 != 0;
        let kind = data[0] & 0x7F;
        let payload = &data[1..];
        match kind {
            // Times are only trusted from the hashed area.
            SUBPACKET_CREATION_TIME if hashed => {
                let bytes: [u8; 4] = payload
                    .try_into()
                    .map_err(|_| "malformed creation time".to_string())?;
                *created = Some(u64::from(u32::from_be_bytes(bytes)));
            }
            SUBPACKET_EXPIRATION_TIME if hashed => {
                let bytes: [u8; 4] = payload
                    .try_into()
                    .map_err(|_| "malformed expiration time".to_string())?;
                signature.expires_after = Some(u64::from(u32::from_be_bytes(bytes)));
            }
            SUBPACKET_ISSUER_FINGERPRINT => {
                if payload.len() == 21 && payload[0] == 4 {
                    let mut fpr = [0u8; 20];
                    fpr.copy_from_slice(&payload[1..]);
                    signature.issuer_fingerprint.get_or_insert(fpr);
                }
            }
            SUBPACKET_ISSUER_KEY_ID => {
                if let Ok(id) = <[u8; 8]>::try_from(payload) {
                    signature.issuer_key_id.get_or_insert(id);
                }
            }
            _ if critical && hashed => {
                return Err(format!("unsupported critical signature subpacket {kind}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Parse the primary Ed25519 key from an armored public key block.
fn parse_primary_key(armored: &str) -> Result<PrimaryKey, String> {
    let bytes = dearmor(armored, "PGP PUBLIC KEY BLOCK")?;
    let packets = parse_packets(&bytes)?;
    let (_, body) = packets
        .iter()
        .find(|(tag, _)| *tag == TAG_PUBLIC_KEY)
        .ok_or("no public key packet in pinned key".to_string())?;
    let mut reader = Reader::new(body);
    if reader.u8()? != 4 {
        return Err("pinned key must be a version 4 key".to_string());
    }
    let created = u64::from(reader.u32()?);
    let algorithm = reader.u8()?;
    let public: [u8; 32] = match algorithm {
        ALGO_EDDSA_LEGACY => {
            let oid_len = usize::from(reader.u8()?);
            if reader.take(oid_len)? != ED25519_OID {
                return Err("pinned key is not on the Ed25519 curve".to_string());
            }
            let point = reader.mpi()?;
            match point {
                [0x40, rest @ ..] if rest.len() == 32 => rest.try_into().unwrap_or([0u8; 32]),
                _ => return Err("malformed Ed25519 public point".to_string()),
            }
        }
        ALGO_ED25519 => reader
            .take(32)?
            .try_into()
            .map_err(|_| "malformed Ed25519 key".to_string())?,
        _ => return Err("pinned key is not an Ed25519 key".to_string()),
    };
    Ok(PrimaryKey {
        algorithm,
        created,
        public,
    })
}

/// Decode an ASCII-armored block of the given kind, verifying its CRC24
/// checksum when present.
fn dearmor(text: &str, kind: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let mut lines = text.lines().map(str::trim_end);
    if !lines.by_ref().any(|line| line == begin) {
        return Err(format!("missing {kind} armor header"));
    }
    // Armor headers ("Comment: ...") end at the first blank line; a block
    // without headers starts its base64 body immediately.
    let mut body = String::new();
    let mut checksum = None;
    let mut in_headers = true;
    let mut finished = false;
    for line in lines {
        if line == end {
            finished = true;
            break;
        }
        if in_headers {
            if line.is_empty() {
                in_headers = false;
                continue;
            }
            if line.contains(": ") {
                continue;
            }
            in_headers = false;
        }
        if let Some(crc) = line.strip_prefix('=') {
            checksum = Some(crc.to_string());
        } else {
            body.push_str(line.trim());
        }
    }
    if !finished {
        return Err(format!("missing {kind} armor footer"));
    }
    let bytes = base64_decode(&body)?;
    if let Some(checksum) = checksum {
        let expected = base64_decode(&checksum)?;
        let crc = crc24(&bytes).to_be_bytes();
        if expected.as_slice() != &crc[1..] {
            return Err("armor checksum mismatch".to_string());
        }
    }
    Ok(bytes)
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let trimmed = text.trim_end_matches('=');
    if !text.len().is_multiple_of(4) || text.len() - trimmed.len() > 2 {
        return Err("invalid base64 length".to_string());
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in trimmed.bytes() {
        buffer = (buffer << 6) | value(byte).ok_or("invalid base64 character".to_string())?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    if buffer != 0 {
        return Err("non-canonical base64 padding".to_string());
    }
    Ok(out)
}

fn crc24(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xB704CE;
    for &byte in bytes {
        crc ^= u32::from(byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= 0x0186_4CFB;
            }
        }
    }
    crc & 0x00FF_FFFF
}

#[cfg(test)]
mod tests {
    //! Failure modes (each must be rejected), then the accepted cases:
    //! tampered data; signature from an unpinned key; SHA-1 digest; text-mode
    //! signature; expired signature; signature created outside the pinned
    //! validity window or before the key existed; truncated, garbled, or
    //! multi-packet input; wrong armor kind; bad armor checksum.
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/openpgp/");
    const NOW: u64 = 1_800_000_000;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap()
    }

    fn manifest() -> Vec<u8> {
        std::fs::read(format!("{FIXTURES}manifest.txt")).unwrap()
    }

    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    fn test_key(valid_until: u64) -> PinnedKey {
        PinnedKey {
            fingerprint: leak(fixture("test-key.fpr").trim().to_string()),
            armored: leak(fixture("test-key.asc")),
            valid_until,
        }
    }

    fn old_key() -> PinnedKey {
        PinnedKey {
            fingerprint: leak(fixture("old-key.fpr").trim().to_string()),
            armored: leak(fixture("old-key.asc")),
            valid_until: u64::MAX,
        }
    }

    fn verify(sig: &str, keys: &[PinnedKey]) -> Result<(), String> {
        verify_detached(&manifest(), &fixture(sig), keys, NOW)
    }

    #[test]
    fn accepts_sha256_and_sha512_signatures() {
        verify("good-sha256.asc", &[test_key(u64::MAX)]).unwrap();
        verify("good-sha512.asc", &[test_key(u64::MAX)]).unwrap();
    }

    #[test]
    fn accepts_when_any_pinned_key_matches() {
        verify("good-sha256.asc", &[old_key(), test_key(u64::MAX)]).unwrap();
    }

    #[test]
    fn rejects_tampered_data() {
        let mut data = manifest();
        data[0] ^= 1;
        let error = verify_detached(
            &data,
            &fixture("good-sha256.asc"),
            &[test_key(u64::MAX)],
            NOW,
        )
        .unwrap_err();
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn rejects_unpinned_key() {
        let error = verify("other-key.asc", &[test_key(u64::MAX)]).unwrap_err();
        assert!(error.contains("pinned"), "{error}");
    }

    #[test]
    fn rejects_sha1_digest() {
        assert!(verify("sha1.asc", &[test_key(u64::MAX)])
            .unwrap_err()
            .contains("SHA-256"));
    }

    #[test]
    fn rejects_text_mode_signature() {
        assert!(verify("textmode.asc", &[test_key(u64::MAX)])
            .unwrap_err()
            .contains("binary"));
    }

    #[test]
    fn rejects_expired_signature() {
        assert!(verify("expired.asc", &[old_key()])
            .unwrap_err()
            .contains("expired"));
        // The same key's non-expiring 2020 signature is still valid.
        verify("old-key-2020.asc", &[old_key()]).unwrap();
    }

    #[test]
    fn rejects_signature_after_validity_window() {
        let error = verify("good-sha256.asc", &[test_key(1_600_000_000)]).unwrap_err();
        assert!(error.contains("validity window"), "{error}");
    }

    #[test]
    fn rejects_truncated_and_garbled_input() {
        let good = fixture("good-sha256.asc");
        let keys = [test_key(u64::MAX)];
        let lines: Vec<&str> = good.lines().collect();
        // Drop one base64 body line.
        let truncated: String = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 2)
            .map(|(_, l)| format!("{l}\n"))
            .collect();
        assert!(verify_detached(&manifest(), &truncated, &keys, NOW).is_err());
        assert!(verify_detached(&manifest(), "garbage", &keys, NOW).is_err());
        assert!(verify_detached(&manifest(), "", &keys, NOW).is_err());
        let wrong_kind = good.replace("PGP SIGNATURE", "PGP MESSAGE");
        assert!(verify_detached(&manifest(), &wrong_kind, &keys, NOW).is_err());
    }

    #[test]
    fn rejects_bad_armor_checksum() {
        let good = fixture("good-sha256.asc");
        let corrupted: String = good
            .lines()
            .map(|line| {
                if let Some(crc) = line.strip_prefix('=') {
                    let flipped = if crc.starts_with('A') { "B" } else { "A" };
                    format!("={flipped}{}\n", &crc[1..])
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        assert!(verify_detached(&manifest(), &corrupted, &[test_key(u64::MAX)], NOW).is_err());
    }

    #[test]
    fn rejects_multiple_signature_packets() {
        let one = dearmor(&fixture("good-sha256.asc"), "PGP SIGNATURE").unwrap();
        let doubled = [one.clone(), one].concat();
        let armored = format!(
            "-----BEGIN PGP SIGNATURE-----\n\n{}\n-----END PGP SIGNATURE-----\n",
            base64_encode(&doubled)
        );
        let error = verify_detached(&manifest(), &armored, &[test_key(u64::MAX)], NOW).unwrap_err();
        assert!(error.contains("exactly one"), "{error}");
    }

    #[test]
    fn real_release_key_parses_as_ed25519_primary() {
        let key = parse_primary_key(include_str!("../release-signing-key.asc")).unwrap();
        assert_eq!(key.algorithm, ALGO_EDDSA_LEGACY);
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
