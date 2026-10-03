#!/usr/bin/env bash
set -euo pipefail

# macOS Developer ID codesigning for deoxidizer binaries.
# Usage: ./scripts/macos-codesign.sh <binary> [binary2 ...]
#
# Requires APPLE_SIGNING_IDENTITY in the environment.
# Use --allow-adhoc only for local, non-release builds.

ALLOW_ADHOC=0
BINARIES=()
for argument in "$@"; do
    if [[ "$argument" == "--allow-adhoc" ]]; then
        ALLOW_ADHOC=1
    else
        BINARIES+=("$argument")
    fi
done

if [[ ${#BINARIES[@]} -eq 0 ]]; then
    echo "Usage: $0 <binary> [binary2 ...]" >&2
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

if [[ -n "${APPLE_TEAM_ID:-}" ]]; then
    echo "Signing with identity: $IDENTITY (Team: $APPLE_TEAM_ID)"
else
    echo "Signing with identity: $IDENTITY"
fi

for bin in "${BINARIES[@]}"; do
    echo "Codesigning $bin..."
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$bin"
    codesign --verify --deep --strict --verbose=2 "$bin"
    echo "Verified: $bin"
done

# Notarize each binary with a preconfigured notarytool keychain profile.
# Standalone Mach-O binaries cannot carry a stapled ticket, so each binary is
# zipped individually for submission; Gatekeeper verifies online. Releases
# require notarization: skipping it needs DEOX_ALLOW_UNNOTARIZED=1 plus
# DEOX_RELEASE_CONFIRM=YES (local staging only).
if [[ -z "${APPLE_KEYCHAIN_PROFILE:-}" ]]; then
    if [[ "${DEOX_ALLOW_UNNOTARIZED:-0}" == "1" && "${DEOX_RELEASE_CONFIRM:-}" == "YES" ]]; then
        echo "APPLE_KEYCHAIN_PROFILE not set; skipping notarization (explicitly allowed)."
        exit 0
    fi
    echo "APPLE_KEYCHAIN_PROFILE is required for notarization; configure it with xcrun notarytool store-credentials." >&2
    echo "(Local staging only: DEOX_ALLOW_UNNOTARIZED=1 DEOX_RELEASE_CONFIRM=YES skips it.)" >&2
    exit 1
fi

PROFILE="$APPLE_KEYCHAIN_PROFILE"
NOTARIZE_TEMPS=()
cleanup_notarize_temps() {
    if (( ${#NOTARIZE_TEMPS[@]} )); then
        for temp in "${NOTARIZE_TEMPS[@]}"; do
            [[ -n "$temp" ]] && rm -rf "$temp"
        done
    fi
}
trap cleanup_notarize_temps EXIT
for bin in "${BINARIES[@]}"; do
    TEMP_DIR="$(mktemp -d -t deoxidizer-notarize.XXXXXX)"
    NOTARIZE_TEMPS+=("$TEMP_DIR")
    ZIP_PATH="$TEMP_DIR/payload.zip"
    ditto -c -k --keepParent "$bin" "$ZIP_PATH"
    echo "Submitting $bin for notarization..."
    # notarytool can exit 0 for a finished-but-rejected submission, so the
    # final status is parsed and must be exactly "Accepted".
    RESULT="$(xcrun notarytool submit "$ZIP_PATH" \
        --keychain-profile "$PROFILE" \
        --wait \
        --output-format json)"
    echo "$RESULT"
    STATUS="$(printf '%s' "$RESULT" | plutil -extract status raw -o - - 2>/dev/null || true)"
    if [[ "$STATUS" != "Accepted" ]]; then
        SUBMISSION_ID="$(printf '%s' "$RESULT" | plutil -extract id raw -o - - 2>/dev/null || true)"
        echo "Notarization failed for $bin (status: ${STATUS:-unknown})." >&2
        if [[ -n "$SUBMISSION_ID" ]]; then
            xcrun notarytool log "$SUBMISSION_ID" --keychain-profile "$PROFILE" >&2 || true
        fi
        exit 1
    fi
    echo "Notarization accepted: $bin"
done
