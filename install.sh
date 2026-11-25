#!/usr/bin/env bash
# deoxidizer installer for macOS and Linux.
#
# Usage:
#   From cloned repo:  ./install.sh --from-source
#   From the web:      curl -fsSL https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.sh | bash
#
# Everything runs inside main(), invoked on the last line, so a download
# cut off mid-transfer can never execute a partial script.

set -euo pipefail

BINARY_NAME="deoxidizer"
ALIAS_NAME="deox"
REPO="BurntToasters/deoxidizer"
RELEASE_MINISIGN_PUBLIC_KEY=$'untrusted comment: minisign public key: 083B254A48C428E3\nRWTjKMRISiU7CDbPKID/yjE1xQvWSbhYFjDF3xgaJCAqOiO7IPffxndk\n'
CURL=(curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --retry-delay 2 --connect-timeout 10 --max-time 300)
DEOX_INSTALL_WORK_DIR=""
DEOX_INSTALL_PAIR_STAGE=""

cleanup_install_staging() {
    if [[ -n "$DEOX_INSTALL_PAIR_STAGE" ]]; then
        rm -rf "$DEOX_INSTALL_PAIR_STAGE"
    fi
    if [[ -n "$DEOX_INSTALL_WORK_DIR" ]]; then
        rm -rf "$DEOX_INSTALL_WORK_DIR"
    fi
}

die() {
    echo "Error: $*" >&2
    exit 1
}

minisign_install_hint() {
    echo "  Install the interoperable Minisign CLI, then run this installer again."
    echo "  (The installer and 'deox --update' verify with the same pinned public key.)"
}

download_release() {
    local work_dir="$1"
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    case "$os" in
        darwin|linux) ;;
        *) die "unsupported operating system: $os" ;;
    esac
    case "$arch" in
        arm64|aarch64) arch="aarch64" ;;
        x86_64|amd64) arch="x86_64" ;;
        *) die "unsupported architecture: $arch" ;;
    esac
    command -v curl >/dev/null 2>&1 || die "curl is required."
    command -v minisign >/dev/null 2>&1 || {
        echo "Error: minisign is required to authenticate the release." >&2
        minisign_install_hint >&2
        exit 1
    }

    # The latest-release redirect names the tag; no JSON parsing needed.
    local latest_url tag version
    latest_url=$("${CURL[@]}" -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")
    tag="${latest_url##*/}"
    version="${tag#v}"
    # Validate before interpolating into URLs (mirrors sync-version.cjs).
    [[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(alpha|beta|rc)\.(0|[1-9][0-9]*))?$ ]] ||
        die "no valid release found (got '$tag')."

    local base="https://github.com/$REPO/releases/download/v$version"
    local asset="deoxidizer-v${version}-${os}-${arch}.tar.gz"
    local checksum_name="SHA256SUMS-${os}-${arch}.txt"
    echo "  Version: v$version"
    echo "  Asset:   $asset"

    "${CURL[@]}" "$base/$asset" -o "$work_dir/$asset"
    if ! "${CURL[@]}" "$base/$checksum_name" -o "$work_dir/$checksum_name"; then
        checksum_name="SHA256SUMS.txt"
        "${CURL[@]}" "$base/$checksum_name" -o "$work_dir/$checksum_name"
    fi
    "${CURL[@]}" "$base/$checksum_name.minisig" -o "$work_dir/$checksum_name.minisig"
    printf '%s' "$RELEASE_MINISIGN_PUBLIC_KEY" > "$work_dir/release-signing-key.pub"
    minisign -V -m "$work_dir/$checksum_name" \
        -x "$work_dir/$checksum_name.minisig" \
        -p "$work_dir/release-signing-key.pub" ||
        die "checksum manifest signature verification failed."

    echo "  Verifying SHA256..."
    local checksum_line expected actual
    checksum_line=$(awk -v asset="$asset" '{ name = $2; sub(/^\*/, "", name); if (name == asset) { print; count++ } } END { exit(count == 1 ? 0 : 1) }' \
        "$work_dir/$checksum_name") || die "$checksum_name has no exact entry for $asset."
    expected=$(awk '{print tolower($1)}' <<<"$checksum_line")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$work_dir/$asset" | awk '{print tolower($1)}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$work_dir/$asset" | awk '{print tolower($1)}')
    else
        die "sha256sum or shasum is required."
    fi
    [[ "$expected" == "$actual" ]] || die "SHA256 mismatch for $asset."

    mkdir -p "$work_dir/extracted"
    tar -xzf "$work_dir/$asset" -C "$work_dir/extracted"
    cp "$work_dir/extracted/$BINARY_NAME" "$work_dir/$BINARY_NAME"
}

build_from_source() {
    local work_dir="$1"
    if [[ ! -f "Cargo.toml" ]] || ! grep -q '^name = "deoxidizer"' Cargo.toml; then
        die "--from-source requires the deoxidizer repository root."
    fi
    echo "Building from source..."
    cargo build --release --locked
    local target_root="${CARGO_TARGET_DIR:-target}"
    cp "$target_root/release/$BINARY_NAME" "$work_dir/$BINARY_NAME"
}

main() {
    local from_source=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --from-source) from_source=1 ;;
            --release) from_source=0 ;;
            *)
                echo "Unknown option: $1" >&2
                exit 2
                ;;
        esac
        shift
    done

    local install_dir
    if [[ $(id -u) -eq 0 ]]; then
        install_dir="/usr/local/bin"
    else
        install_dir="${HOME}/.local/bin"
    fi

    local work_dir
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/deoxidizer-install.XXXXXX")
    DEOX_INSTALL_WORK_DIR="$work_dir"
    trap cleanup_install_staging EXIT
    [[ ! -L "$install_dir" ]] || die "install directory is an unexpected symlink: $install_dir"
    mkdir -p "$install_dir"
    [[ -d "$install_dir" && ! -L "$install_dir" ]] || die "install path is not a real directory: $install_dir"

    echo "🔧 deoxidizer installer"
    echo "────────────────────────"

    if [[ "$from_source" -eq 1 ]]; then
        build_from_source "$work_dir"
    else
        echo "Downloading latest release from GitHub..."
        download_release "$work_dir"
    fi

    local staged="$work_dir/$BINARY_NAME"
    [[ -f "$staged" && ! -L "$staged" ]] || die "staged binary missing or symlinked."
    local binary_dest="$install_dir/$BINARY_NAME"
    local alias_dest="$install_dir/$ALIAS_NAME"
    if [[ -L "$binary_dest" || ( -e "$binary_dest" && ! -f "$binary_dest" ) ]]; then
        die "install destination is not a regular binary file: $binary_dest"
    fi
    if [[ -L "$alias_dest" ]]; then
        [[ "$(readlink "$alias_dest")" == "$binary_dest" ]] ||
            die "install destination is an unexpected symlink: $alias_dest"
    elif [[ -e "$alias_dest" && ! -f "$alias_dest" ]]; then
        die "install destination is not a regular file: $alias_dest"
    fi

    # Stage both invokers on the destination filesystem. Install the alias
    # first while it still resolves to the old main binary, then atomically
    # rename the new main binary into place. A crash or main-file failure
    # therefore leaves both invokers on the old bytes; success leaves both
    # invokers on the new bytes.
    DEOX_INSTALL_PAIR_STAGE=$(mktemp -d "$install_dir/.deoxidizer-install.XXXXXX")
    install -m 0755 "$staged" "$DEOX_INSTALL_PAIR_STAGE/$BINARY_NAME"
    ln -s "$binary_dest" "$DEOX_INSTALL_PAIR_STAGE/$ALIAS_NAME"
    mv -f "$DEOX_INSTALL_PAIR_STAGE/$ALIAS_NAME" "$alias_dest"
    mv -f "$DEOX_INSTALL_PAIR_STAGE/$BINARY_NAME" "$binary_dest"
    rmdir "$DEOX_INSTALL_PAIR_STAGE"
    DEOX_INSTALL_PAIR_STAGE=""

    echo "✓ Installed $BINARY_NAME to $install_dir/$BINARY_NAME"
    echo "✓ Linked $ALIAS_NAME -> $install_dir/$BINARY_NAME"

    if ! tr ':' '\n' <<<"$PATH" | grep -qx "$install_dir"; then
        echo ""
        echo "⚠  $install_dir is not in your PATH."
        local shell_name rc
        shell_name=$(basename "${SHELL:-sh}")
        case "$shell_name" in
            zsh)  rc="$HOME/.zshrc" ;;
            bash) rc="$HOME/.bashrc" ;;
            fish) rc="$HOME/.config/fish/config.fish" ;;
            *)    rc="your shell profile" ;;
        esac
        echo "   Add this to $rc:"
        if [[ "$shell_name" == "fish" ]]; then
            echo "     fish_add_path $install_dir"
        else
            echo "     export PATH=\"$install_dir:\$PATH\""
        fi
        echo ""
    fi

    echo ""
    echo "Run 'deox setup' to configure, or 'deox scan' to get started."
}

main "$@"
