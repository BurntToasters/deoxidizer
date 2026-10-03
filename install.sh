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
RELEASE_KEY_FINGERPRINT="CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F"
CURL=(curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --retry-delay 2 --connect-timeout 10 --max-time 300)

die() {
    echo "Error: $*" >&2
    exit 1
}

gpg_install_hint() {
    case "$(uname -s)" in
        Darwin) echo "  Install it with: brew install gnupg" ;;
        Linux)
            if command -v apt-get >/dev/null 2>&1; then
                echo "  Install it with: sudo apt-get install gnupg"
            elif command -v dnf >/dev/null 2>&1; then
                echo "  Install it with: sudo dnf install gnupg2"
            elif command -v pacman >/dev/null 2>&1; then
                echo "  Install it with: sudo pacman -S gnupg"
            else
                echo "  Install GnuPG with your package manager."
            fi
            ;;
    esac
    echo "  (Only this first install needs gpg; 'deox --update' verifies releases itself.)"
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
    command -v gpg >/dev/null 2>&1 || {
        echo "Error: gpg is required to authenticate the release." >&2
        gpg_install_hint >&2
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
    "${CURL[@]}" "$base/$checksum_name.asc" -o "$work_dir/$checksum_name.asc"

    # Verify the manifest signature with an isolated keyring holding only
    # the pinned release key.
    local gnupg_home="$work_dir/gnupg"
    mkdir -m 700 "$gnupg_home"
    "${CURL[@]}" "https://raw.githubusercontent.com/$REPO/main/release-signing-key.asc" \
        -o "$work_dir/release-signing-key.asc"
    local key_fingerprint
    key_fingerprint=$(gpg --homedir "$gnupg_home" --batch --show-keys --with-colons "$work_dir/release-signing-key.asc" |
        awk -F: '$1 == "fpr" { print toupper($10); exit }')
    [[ "$key_fingerprint" == "$RELEASE_KEY_FINGERPRINT" ]] || die "release signing key fingerprint mismatch."
    gpg --homedir "$gnupg_home" --batch --yes --dearmor --output "$work_dir/release-keyring.gpg" \
        "$work_dir/release-signing-key.asc"
    local status
    status=$(gpg --homedir "$gnupg_home" --batch --no-options --no-default-keyring \
        --keyring "$work_dir/release-keyring.gpg" --status-fd 1 \
        --verify "$work_dir/$checksum_name.asc" "$work_dir/$checksum_name" 2>/dev/null) ||
        die "checksum manifest signature verification failed."
    grep -q "^\[GNUPG:\] VALIDSIG $RELEASE_KEY_FINGERPRINT " <<<"$status" ||
        die "checksum manifest is not signed by the pinned release key."

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
    # shellcheck disable=SC2064 # expand now: work_dir is local to main
    trap "rm -rf '$work_dir'" EXIT
    mkdir -p "$install_dir"

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
    for name in "$BINARY_NAME" "$ALIAS_NAME"; do
        local dest="$install_dir/$name"
        if [[ -L "$dest" ]]; then
            [[ "$name" == "$ALIAS_NAME" && "$(readlink "$dest")" == "$install_dir/$BINARY_NAME" ]] ||
                die "install destination is an unexpected symlink: $dest"
        fi
    done
    install -m 0755 "$staged" "$install_dir/$BINARY_NAME"
    # `deox` is a symlink to the single installed binary.
    rm -f "$install_dir/$ALIAS_NAME"
    ln -s "$install_dir/$BINARY_NAME" "$install_dir/$ALIAS_NAME"

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
