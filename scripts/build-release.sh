#!/usr/bin/env bash
set -euo pipefail

# Build release binaries for one explicit Rust target.
# Usage: ./scripts/build-release.sh [--target <triple>] [--output-dir <dir>]

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Cargo.toml version is parsed with a section-aware match so a dependency's
# `version =` line can never shadow the package version. (Shell reuse of
# sync-version.cjs via node is impractical here because this script must run
# before node_modules exists; the awk below mirrors cargoVersion().)
VERSION=$(awk '
    /^\[package\]/{ in_package=1; next }
    /^\[/{ in_package=0 }
    in_package && /^version[[:space:]]*=[[:space:]]*"/ {
        gsub(/^version[[:space:]]*=[[:space:]]*"/, "");
        gsub(/".*$/, "");
        print; exit
    }
' Cargo.toml | head -n 1)
echo "Building deoxidizer v$VERSION..."

TARGET="$(rustc -vV | sed -n 's/^host: //p')"
OUTPUT_DIR="$ROOT_DIR/release"
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
    case $1 in
        --target)
            [[ $# -ge 2 ]] || { echo "--target requires a value" >&2; exit 2; }
            TARGET="$2"
            shift 2
            ;;
        --output-dir)
            [[ $# -ge 2 ]] || { echo "--output-dir requires a value" >&2; exit 2; }
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

if [[ "$OUTPUT_DIR" != /* ]]; then
    OUTPUT_DIR="$ROOT_DIR/$OUTPUT_DIR"
fi

CARGO_ARGS=(build --release --locked)
CARGO_ARGS+=(--target "$TARGET")
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    cargo "${CARGO_ARGS[@]}"
fi

# Cargo places explicit-target builds under target/<triple>/release.
CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
OUT_DIR="$CARGO_TARGET_ROOT/$TARGET/release"
case "$TARGET" in
    x86_64-unknown-linux-gnu)
        OS="linux"; ARCH="x86_64"; FORMAT="tar.gz"; BIN_EXT="" ;;
    aarch64-unknown-linux-gnu)
        OS="linux"; ARCH="aarch64"; FORMAT="tar.gz"; BIN_EXT="" ;;
    x86_64-apple-darwin)
        OS="darwin"; ARCH="x86_64"; FORMAT="tar.gz"; BIN_EXT="" ;;
    aarch64-apple-darwin)
        OS="darwin"; ARCH="aarch64"; FORMAT="tar.gz"; BIN_EXT="" ;;
    x86_64-pc-windows-msvc|x86_64-pc-windows-gnu)
        OS="windows"; ARCH="x86_64"; FORMAT="zip"; BIN_EXT=".exe" ;;
    aarch64-pc-windows-msvc)
        OS="windows"; ARCH="aarch64"; FORMAT="zip"; BIN_EXT=".exe" ;;
    *)
        echo "Unsupported release target: $TARGET" >&2
        exit 1
        ;;
esac

[[ -f "$OUT_DIR/deoxidizer$BIN_EXT" ]] || {
    echo "Missing release binary: $OUT_DIR/deoxidizer$BIN_EXT" >&2
    exit 1
}
[[ -f "$OUT_DIR/deox$BIN_EXT" ]] || {
    echo "Missing release binary: $OUT_DIR/deox$BIN_EXT" >&2
    exit 1
}

mkdir -p "$OUTPUT_DIR"
ARCHIVE="$OUTPUT_DIR/deoxidizer-v${VERSION}-${OS}-${ARCH}.${FORMAT}"
rm -f "$ARCHIVE"
STAGE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/deoxidizer-release.XXXXXX")
trap 'rm -rf "$STAGE_DIR"' EXIT
cp "$OUT_DIR/deoxidizer$BIN_EXT" "$STAGE_DIR/"
cp "$OUT_DIR/deox$BIN_EXT" "$STAGE_DIR/"
cp "$ROOT_DIR/LICENSE" "$STAGE_DIR/"
touch -t 198001010000 "$STAGE_DIR"/*

echo "Packaging $ARCHIVE..."
if [[ "$FORMAT" == "tar.gz" ]]; then
    TAR_PATH="$STAGE_DIR/deoxidizer.tar"
    if tar --version 2>/dev/null | grep -q 'GNU tar'; then
        tar -cf "$TAR_PATH" \
            --sort=name --owner=0 --group=0 --numeric-owner \
            --mtime="@${SOURCE_DATE_EPOCH:-0}" -C "$STAGE_DIR" \
            "deoxidizer$BIN_EXT" "deox$BIN_EXT" LICENSE
    else
        # BSD-tar fallback skips --mtime/--owner/--sort (nondeterministic
        # macOS bytes). Accepted: single-producer staging keeps this harmless;
        # COPYFILE_DISABLE + touch -t above still normalize xattrs/mtimes.
        COPYFILE_DISABLE=1 tar -cf "$TAR_PATH" -C "$STAGE_DIR" \
            "deoxidizer$BIN_EXT" "deox$BIN_EXT" LICENSE
    fi
    gzip -n -c "$TAR_PATH" > "$ARCHIVE"
else
    (cd "$STAGE_DIR" && zip -X -q "$ARCHIVE" "deoxidizer$BIN_EXT" "deox$BIN_EXT" LICENSE)
fi

echo "  Created $ARCHIVE"
echo "Release artifacts in $OUTPUT_DIR"
ls -lh "$OUTPUT_DIR"
