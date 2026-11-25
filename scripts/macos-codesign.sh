#!/usr/bin/env bash
set -euo pipefail

# macOS Developer ID signing for deoxidizer binaries.
# Usage: ./scripts/macos-codesign.sh --proof-file <path> --target <triple> --commit <sha> <binary> <binary>

PROOF_FILE=""
TARGET=""
COMMIT=""
ALLOW_ADHOC=0
BINARIES=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --proof-file)
            [[ $# -ge 2 ]] || { echo "--proof-file requires a value" >&2; exit 2; }
            PROOF_FILE="$2"; shift 2 ;;
        --target)
            [[ $# -ge 2 ]] || { echo "--target requires a value" >&2; exit 2; }
            TARGET="$2"; shift 2 ;;
        --commit)
            [[ $# -ge 2 ]] || { echo "--commit requires a value" >&2; exit 2; }
            COMMIT="$2"; shift 2 ;;
        --allow-adhoc)
            ALLOW_ADHOC=1; shift ;;
        --*) echo "Unknown option: $1" >&2; exit 2 ;;
        *) BINARIES+=("$1"); shift ;;
    esac
done

if [[ ${#BINARIES[@]} -ne 2 || -z "$PROOF_FILE" || -z "$TARGET" || ! "$COMMIT" =~ ^[0-9a-fA-F]{40}$ ]]; then
    echo "Usage: $0 --proof-file <path> --target <triple> --commit <sha> <binary> <binary>" >&2
    exit 1
fi

IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
if [[ -z "$IDENTITY" || "$IDENTITY" == "-" ]]; then
    if [[ "$ALLOW_ADHOC" -ne 1 ]]; then
        echo "APPLE_SIGNING_IDENTITY is required; pass --allow-adhoc only for local builds." >&2
        exit 1
    fi
    echo "APPLE_SIGNING_IDENTITY not set; ad-hoc signing explicitly allowed."
    for bin in "${BINARIES[@]}"; do
        echo "Ad-hoc signing $bin..."
        codesign --force --sign - "$bin"
    done
    exit 0
fi

TEAM_ID="${APPLE_TEAM_ID:-}"
if [[ -z "$TEAM_ID" ]]; then
    echo "APPLE_TEAM_ID is required to verify the Developer ID signature." >&2
    exit 1
fi
if [[ -z "${APPLE_KEYCHAIN_PROFILE:-}" ]]; then
    if [[ "${DEOX_ALLOW_UNNOTARIZED:-0}" == "1" && "${DEOX_RELEASE_CONFIRM:-}" == "YES" ]]; then
        echo "APPLE_KEYCHAIN_PROFILE not set; skipping notarization for local staging only." >&2
        exit 0
    fi
    echo "APPLE_KEYCHAIN_PROFILE is required for notarization; configure it with xcrun notarytool store-credentials." >&2
    exit 1
fi

echo "Signing with identity: $IDENTITY (Team: $TEAM_ID)"
for bin in "${BINARIES[@]}"; do
    [[ -f "$bin" && ! -L "$bin" ]] || { echo "Release binary is not a regular file: $bin" >&2; exit 1; }
    echo "Codesigning $bin..."
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$bin"
    codesign --verify --deep --strict --verbose=2 "$bin"
    DETAILS="$(codesign --display --verbose=4 "$bin" 2>&1)" || {
        echo "Cannot inspect the codesign details for $bin" >&2
        exit 1
    }
    ACTUAL_TEAM="$(printf '%s\n' "$DETAILS" | sed -n 's/^TeamIdentifier=//p' | head -n 1)"
    TIMESTAMP="$(printf '%s\n' "$DETAILS" | sed -n 's/^Timestamp=//p' | head -n 1)"
    if [[ "$ACTUAL_TEAM" != "$TEAM_ID" ]]; then
        echo "Unexpected Team ID for $bin: ${ACTUAL_TEAM:-<missing>}" >&2
        exit 1
    fi
    if [[ -z "$TIMESTAMP" || "$TIMESTAMP" == "none" ]]; then
        echo "Missing secure code-signing timestamp for $bin" >&2
        exit 1
    fi
    echo "Verified timestamped Developer ID signature: $bin"
done

PROFILE="$APPLE_KEYCHAIN_PROFILE"
NOTARIZE_TEMPS=()
RECORDS_FILE="$(mktemp "${TMPDIR:-/tmp}/deoxidizer-notary-records.XXXXXX")"
cleanup_notarize_temps() {
    for temp in "${NOTARIZE_TEMPS[@]}"; do
        [[ -n "$temp" ]] && rm -rf "$temp"
    done
    [[ -n "$RECORDS_FILE" ]] && rm -f "$RECORDS_FILE"
}
trap cleanup_notarize_temps EXIT
for bin in "${BINARIES[@]}"; do
    TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/deoxidizer-notarize.XXXXXX")"
    NOTARIZE_TEMPS+=("$TEMP_DIR")
    ZIP_PATH="$TEMP_DIR/payload.zip"
    ditto -c -k --keepParent "$bin" "$ZIP_PATH"
    echo "Submitting $bin for notarization..."
    RESULT="$(xcrun notarytool submit "$ZIP_PATH" \
        --keychain-profile "$PROFILE" \
        --wait \
        --output-format json)"
    STATUS="$(printf '%s' "$RESULT" | plutil -extract status raw -o - - 2>/dev/null || true)"
    SUBMISSION_ID="$(printf '%s' "$RESULT" | plutil -extract id raw -o - - 2>/dev/null || true)"
    if [[ "$STATUS" != "Accepted" || -z "$SUBMISSION_ID" ]]; then
        echo "Notarization failed for $bin (status: ${STATUS:-unknown})." >&2
        if [[ -n "$SUBMISSION_ID" ]]; then
            xcrun notarytool log "$SUBMISSION_ID" --keychain-profile "$PROFILE" >&2 || true
        fi
        exit 1
    fi
    printf '%s\t%s\n' "$(basename "$bin")" "$SUBMISSION_ID" >> "$RECORDS_FILE"
    echo "Notarization accepted: $bin ($SUBMISSION_ID)"
done

node - "$PROOF_FILE" "$TARGET" "$COMMIT" "$TEAM_ID" "$RECORDS_FILE" "${BINARIES[@]}" <<'NODE'
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const [proofPath, target, commit, teamId, recordsPath, ...binaries] = process.argv.slice(2);
const digest = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const submissions = fs.readFileSync(recordsPath, 'utf8').trim().split(/\r?\n/).filter(Boolean).map((line) => {
  const [asset, submission_id] = line.split('\t');
  if (!asset || !submission_id) throw new Error('Invalid notarization submission record');
  return { asset: path.parse(asset).name, submission_id };
});
if (submissions.length !== binaries.length || binaries.length !== 2) {
  throw new Error('Notarization proof does not cover both release binaries');
}
const proof = {
  schema_version: 1,
  platform: 'darwin',
  target,
  commit,
  artifacts: Object.fromEntries(binaries.map((file) => [path.basename(file), digest(file)])),
  native: {
    mechanism: 'codesign',
    signature_verified: true,
    timestamp_verified: true,
    team_id: teamId,
    notarization: { status: 'Accepted', submissions },
  },
};
fs.writeFileSync(proofPath, `${JSON.stringify(proof, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
NODE
