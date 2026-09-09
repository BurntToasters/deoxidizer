#!/usr/bin/env bash
set -euo pipefail

# GPG signing & checksum generation for deoxidizer release artifacts.
# Usage: ./scripts/gpg-sign.sh [release-dir] [--allow-unsigned]
#
# Generates a target-scoped SHA256SUMS*.txt and .asc signatures using GPG_KEY_ID.

DIR="release"
ALLOW_UNSIGNED=0
for argument in "$@"; do
    case "$argument" in
        --allow-unsigned) ALLOW_UNSIGNED=1 ;;
        release|.) DIR="$argument" ;;
        --*) echo "Unknown option: $argument" >&2; exit 2 ;;
        *) DIR="$argument" ;;
    esac
done

if [[ ! -d "$DIR" ]]; then
    echo "Release directory not found: $DIR" >&2
    exit 1
fi

cd "$DIR"

shopt -s nullglob

ARTIFACTS=(deoxidizer-*.tar.gz deoxidizer-*.zip deoxidizer-*-setup.exe)
if [[ ${#ARTIFACTS[@]} -eq 0 ]]; then
    echo "No release archives found in $DIR." >&2
    exit 1
fi
CHECKSUM_NAME="${DEOX_CHECKSUM_NAME:-SHA256SUMS.txt}"
# Canonical manifest names are case-sensitive: uppercase SHA256SUMS prefix
# with a lowercase `-<os>-<arch>` suffix (e.g. SHA256SUMS-linux-x86_64.txt).
# Keep the match strict on purpose; verifiers accept legacy SHA256SUMS.txt
# as a fallback but release.cjs always writes the per-target canonical name.
[[ "$CHECKSUM_NAME" =~ ^SHA256SUMS(-[a-z0-9_-]+)?\.txt$ ]] || {
    echo "Invalid checksum manifest name: $CHECKSUM_NAME" >&2
    exit 2
}
echo "Generating $CHECKSUM_NAME..."

if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${ARTIFACTS[@]}" | sed 's#  .*/#  #' > "$CHECKSUM_NAME"
elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${ARTIFACTS[@]}" | sed 's#  .*/#  #' > "$CHECKSUM_NAME"
else
    echo "Neither sha256sum nor shasum is installed." >&2
    exit 1
fi
echo "  $(wc -l < "$CHECKSUM_NAME" | tr -d ' ') entries written to $CHECKSUM_NAME."

if [[ -z "${GPG_KEY_ID:-}" ]]; then
    if [[ "$ALLOW_UNSIGNED" -eq 1 ]]; then
        if [[ "${DEOX_RELEASE_CONFIRM:-}" != "YES" ]]; then
            echo "Unsigned staging requires DEOX_RELEASE_CONFIRM=YES explicitly." >&2
            exit 1
        fi
        echo "GPG_KEY_ID not set; unsigned staging explicitly allowed."
        exit 0
    fi
    echo "GPG_KEY_ID is required for release signing." >&2
    exit 1
fi

# Check gpg is available
if ! command -v gpg &>/dev/null; then
    echo "gpg not found. Install GnuPG to enable signing." >&2
    exit 1
fi

GPG_ARGS=(--batch --yes --armor --detach-sign --local-user "$GPG_KEY_ID")

for file in "${ARTIFACTS[@]}" "$CHECKSUM_NAME"; do
    echo "GPG signing $file -> $file.asc"
    if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
        printf '%s\n' "$GPG_PASSPHRASE" |
            gpg "${GPG_ARGS[@]}" --pinentry-mode loopback --passphrase-fd 0 \
                --output "$file.asc" "$file"
    else
        gpg "${GPG_ARGS[@]}" --output "$file.asc" "$file"
    fi
done

echo "Done. All artifacts signed."
