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

# Notarize if Apple ID credentials are available
if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    for bin in "${BINARIES[@]}"; do
        ZIP_PATH="$(mktemp -t deoxidizer-notarize-XXXXXX).zip"
        ditto -c -k --keepParent "$bin" "$ZIP_PATH"
        echo "Submitting $bin for notarization..."
        xcrun notarytool submit "$ZIP_PATH" \
            --apple-id "$APPLE_ID" \
            --password "$APPLE_PASSWORD" \
            --team-id "$APPLE_TEAM_ID" \
            --wait
        rm -f "$ZIP_PATH"
        echo "Notarization complete: $bin"
    done
fi
