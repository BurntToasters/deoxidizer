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
RELEASE_KEY_FINGERPRINT="CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F"
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

    curl -fsSL --retry 3 --retry-delay 2 "https://api.github.com/repos/$REPO/releases/latest" \
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
    # Validate semver before interpolating into asset names/URLs so a
    # compromised or malformed tag cannot inject path segments.
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+)?$ ]] || {
        echo "Error: invalid release version: $VERSION" >&2
        exit 1
    }
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
    CHECKSUM_SIGNATURE_URL="${CHECKSUM_URL}.asc"

    echo "  Version: $LATEST"
    echo "  Asset:   $ASSET"
    curl -fsSL --retry 3 --retry-delay 2 "$URL" -o "$TMPDIR/$ASSET"
    if ! curl -fsSL --retry 3 --retry-delay 2 "$CHECKSUM_URL" -o "$TMPDIR/$CHECKSUM_NAME"; then
        CHECKSUM_NAME="SHA256SUMS.txt"
        curl -fsSL --retry 3 --retry-delay 2 "https://github.com/$REPO/releases/download/${LATEST}/$CHECKSUM_NAME" \
            -o "$TMPDIR/$CHECKSUM_NAME"
        CHECKSUM_URL="https://github.com/$REPO/releases/download/${LATEST}/${CHECKSUM_NAME}"
        CHECKSUM_SIGNATURE_URL="${CHECKSUM_URL}.asc"
    fi

    command -v gpg >/dev/null 2>&1 || {
        echo "Error: gpg is required to authenticate release manifests." >&2
        exit 1
    }
    curl -fsSL --retry 3 --retry-delay 2 "https://raw.githubusercontent.com/$REPO/main/release-signing-key.asc" \
        -o "$TMPDIR/release-signing-key.asc"
    KEY_FINGERPRINT=$(gpg --batch --show-keys --with-colons "$TMPDIR/release-signing-key.asc" |
        awk -F: '$1 == "fpr" { print toupper($10); exit }')
    [[ "$KEY_FINGERPRINT" == "$RELEASE_KEY_FINGERPRINT" ]] || {
        echo "Error: release signing key fingerprint mismatch." >&2
        exit 1
    }
    gpg --batch --yes --dearmor --output "$TMPDIR/release-keyring.gpg" \
        "$TMPDIR/release-signing-key.asc"
    curl -fsSL --retry 3 --retry-delay 2 "$CHECKSUM_SIGNATURE_URL" -o "$TMPDIR/$CHECKSUM_NAME.asc"
    gpg --batch --no-options --no-default-keyring --keyring "$TMPDIR/release-keyring.gpg" \
        --verify "$TMPDIR/$CHECKSUM_NAME.asc" "$TMPDIR/$CHECKSUM_NAME" >/dev/null 2>&1 || {
        echo "Error: checksum manifest signature verification failed." >&2
        exit 1
    }

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
    if [[ -L "$INSTALL_DIR/$binary" ]]; then
        DEST_TARGET=$(readlink "$INSTALL_DIR/$binary")
        [[ "$binary" == "$ALIAS_NAME" && "$DEST_TARGET" == "$INSTALL_DIR/$BINARY_NAME" ]] || {
            echo "Error: install destination is an unexpected symlink: $INSTALL_DIR/$binary" >&2
            exit 1
        }
        rm -f "$INSTALL_DIR/$binary"
    fi
    chmod 0755 "$TMPDIR/$binary"
    # Overwrite-in-place with no backup is the installer standard: the
    # previous release binary is superseded, never preserved alongside.
    install -m 0755 "$TMPDIR/$binary" "$INSTALL_DIR/$binary"
done

echo "✓ Installed $BINARY_NAME to $INSTALL_DIR/$BINARY_NAME"
echo "✓ Installed $ALIAS_NAME to $INSTALL_DIR/$ALIAS_NAME"

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
