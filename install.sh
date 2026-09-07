#!/usr/bin/env bash
set -euo pipefail

# deoxidizer installer for macOS and Linux.
#
# Usage:
#   From cloned repo:  ./install.sh --from-source
#   From the web:      curl -fsSL https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.sh | bash

BINARY_NAME="deoxidizer"
ALIAS_NAME="deox"
REPO="BurntToasters/deoxidizer"
FROM_SOURCE=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --from-source) FROM_SOURCE=1 ;;
        --release) FROM_SOURCE=0 ;;
        *)
            echo "Unknown option: $1" >&2
            exit 2
            ;;
    esac
    shift
done

# Determine install directory
if [[ $(id -u) -eq 0 ]]; then
    INSTALL_DIR="/usr/local/bin"
else
    INSTALL_DIR="${HOME}/.local/bin"
fi

TMPDIR=$(mktemp -d "${TMPDIR:-/tmp}/deoxidizer-install.XXXXXX")
trap 'rm -rf "$TMPDIR"' EXIT
mkdir -p "$INSTALL_DIR"

echo "🔧 deoxidizer installer"
echo "────────────────────────"

if [[ "$FROM_SOURCE" -eq 1 ]]; then
    if [[ ! -f "Cargo.toml" ]] || ! grep -q '^name = "deoxidizer"' Cargo.toml; then
        echo "Error: --from-source requires deoxidizer repository root." >&2
        exit 1
    fi
    echo "Building from source..."
    cargo build --release --locked
    CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-target}"
    cp "$CARGO_TARGET_ROOT/release/$BINARY_NAME" "$TMPDIR/$BINARY_NAME"
    cp "$CARGO_TARGET_ROOT/release/$ALIAS_NAME" "$TMPDIR/$ALIAS_NAME"
else
    echo "Downloading latest release from GitHub..."
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    ARCH=$(uname -m)
    case "$OS" in
        darwin|linux) ;;
        *) echo "Unsupported operating system: $OS" >&2; exit 1 ;;
    esac
    case "$ARCH" in
        arm64|aarch64) ARCH="aarch64" ;;
        x86_64|amd64) ARCH="x86_64" ;;
        *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac

    command -v curl >/dev/null 2>&1 || {
        echo "Error: curl is required." >&2
        exit 1
    }
    command -v python3 >/dev/null 2>&1 || {
        echo "Error: python3 is required for safe GitHub JSON parsing." >&2
        exit 1
    }

    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        -o "$TMPDIR/release.json"
    VERSION=$(python3 - "$TMPDIR/release.json" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as stream:
    release = json.load(stream)
tag = release.get("tag_name", "")
if not isinstance(tag, str) or not tag:
    raise SystemExit("missing release tag")
print(tag[1:] if tag.startswith("v") else tag)
PY
)
    LATEST="v$VERSION"
    ASSET="deoxidizer-v${VERSION}-${OS}-${ARCH}.tar.gz"
    URL=$(python3 - "$TMPDIR/release.json" "$ASSET" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as stream:
    release = json.load(stream)
for asset in release.get("assets", []):
    if asset.get("name") == sys.argv[2]:
        print(asset["browser_download_url"])
        break
else:
    raise SystemExit(f"release asset not found: {sys.argv[2]}")
PY
)
    [[ "$URL" == "https://github.com/$REPO/releases/download/"* ]] || {
        echo "Error: release asset URL is not an expected GitHub URL." >&2
        exit 1
    }
    CHECKSUM_NAME="SHA256SUMS-${OS}-${ARCH}.txt"
    CHECKSUM_URL="https://github.com/$REPO/releases/download/${LATEST}/${CHECKSUM_NAME}"

    echo "  Version: $LATEST"
    echo "  Asset:   $ASSET"
    curl -fsSL "$URL" -o "$TMPDIR/$ASSET"
    if ! curl -fsSL "$CHECKSUM_URL" -o "$TMPDIR/$CHECKSUM_NAME"; then
        CHECKSUM_NAME="SHA256SUMS.txt"
        curl -fsSL "https://github.com/$REPO/releases/download/${LATEST}/$CHECKSUM_NAME" \
            -o "$TMPDIR/$CHECKSUM_NAME"
    fi

    echo "  Verifying SHA256..."
    CHECKSUM_LINE=$(awk -v asset="$ASSET" '{ name = $2; sub(/^\*/, "", name); if (name == asset) { print; count++ } } END { exit(count == 1 ? 0 : 1) }' \
        "$TMPDIR/$CHECKSUM_NAME") || {
        echo "Error: $CHECKSUM_NAME has no exact entry for $ASSET." >&2
        exit 1
    }
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$TMPDIR" && printf '%s\n' "$CHECKSUM_LINE" | sha256sum -c - >/dev/null)
    elif command -v shasum >/dev/null 2>&1; then
        EXPECTED=$(printf '%s\n' "$CHECKSUM_LINE" | awk '{print $1}')
        ACTUAL=$(shasum -a 256 "$TMPDIR/$ASSET" | awk '{print $1}')
        [[ "$EXPECTED" == "$ACTUAL" ]]
    else
        echo "Error: sha256sum or shasum is required." >&2
        exit 1
    fi

    mkdir -p "$TMPDIR/extracted"
    tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR/extracted"
    cp "$TMPDIR/extracted/$BINARY_NAME" "$TMPDIR/$BINARY_NAME"
    cp "$TMPDIR/extracted/$ALIAS_NAME" "$TMPDIR/$ALIAS_NAME"
fi

for binary in "$BINARY_NAME" "$ALIAS_NAME"; do
    [[ -f "$TMPDIR/$binary" && ! -L "$TMPDIR/$binary" ]] || {
        echo "Error: staged binary missing or symlinked: $binary" >&2
        exit 1
    }
    chmod 0755 "$TMPDIR/$binary"
    install -m 0755 "$TMPDIR/$binary" "$INSTALL_DIR/$binary"
done

# Create deox symlink
ln -sf "$INSTALL_DIR/$BINARY_NAME" "$INSTALL_DIR/$ALIAS_NAME"

echo "✓ Installed $BINARY_NAME to $INSTALL_DIR/$BINARY_NAME"
echo "✓ Created symlink $ALIAS_NAME -> $BINARY_NAME"

# Check if install dir is in PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo ""
    echo "⚠  $INSTALL_DIR is not in your PATH."
    SHELL_NAME=$(basename "$SHELL")
    case "$SHELL_NAME" in
        zsh)  RC="$HOME/.zshrc" ;;
        bash) RC="$HOME/.bashrc" ;;
        fish) RC="$HOME/.config/fish/config.fish" ;;
        *)    RC="your shell profile" ;;
    esac
    echo "   Add this to $RC:"
    if [[ "$SHELL_NAME" == "fish" ]]; then
        echo "     fish_add_path $INSTALL_DIR"
    else
        echo "     export PATH=\"$INSTALL_DIR:\$PATH\""
    fi
    echo ""
fi

echo ""
echo "Run 'deox setup' to configure, or 'deox scan' to get started."
