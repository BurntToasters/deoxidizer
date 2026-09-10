# ⬇️ Downloads

| <img height="20" src="https://raw.githubusercontent.com/BurntToasters/bcls/main/media/windows.png" /> Windows | <img height="20" src="https://raw.githubusercontent.com/BurntToasters/bcls/main/media/mac.png" /> macOS | <img height="20" src="https://raw.githubusercontent.com/BurntToasters/bcls/main/media/linux.png" /> Linux |
| :--- | :--- | :--- |
| **ZIP:** [x64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-x86_64.zip) / [arm64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-aarch64.zip) | **tar.gz:** [x64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-darwin-x86_64.tar.gz) / [arm64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-darwin-aarch64.tar.gz) | **tar.gz:** [x64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-linux-x86_64.tar.gz) / [arm64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-linux-aarch64.tar.gz) |
| **Setup:** [x64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-x86_64-setup.exe) / [arm64](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-aarch64-setup.exe) | | |

> [!IMPORTANT]
> Every release archive includes both `deoxidizer` and `deox`.
>
> Each target has a platform-scoped `SHA256SUMS-<os>-<arch>.txt` manifest. The
> `.asc` files are detached GPG signatures for release verification.
>
> `deox --update` and installers refuse archives without an exact checksum
> entry. Windows setup files require Authenticode verification.
>
> This project is pre-1.0. Bugs, rough edges, and breaking changes are possible.

### ℹ️ Enjoying deoxidizer? Consider [❤️ Supporting Me! ❤️](https://rosie.run/support)

## Changes in `v0.1.0:`

- **NEW - Rust Artifact Cleaner:** Added safe scanning and cleanup for Rust and Tauri `target/` build artifacts.
- **NEW - Cleaning Modes:** Added `full`, `debug-only`, `incremental-only`, and `deps-only` cleanup modes.
- **NEW - Dual Invokers:** Added `deoxidizer` and `deox` commands with shared behavior and update logic.
- **NEW - Workspace Detection:** Added Tauri dependency detection for renamed dependencies, inherited workspace dependencies, and shared workspace targets.
- **Security:** Reject symlinked artifact paths and revalidate target containment before cleanup.
  - Scan, inspect, and dry-run operations remain read-only.
  - Clean operations require confirmation unless `--yes` is explicitly provided.
- **Updater:** Added HTTPS-only release downloads, bounded responses, safe archive extraction, and mandatory platform checksum verification.
- **Updater:** Authenticates checksum manifests with pinned GPG key `CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F` before replacement.
- **Codebase:** Added shared Node.js release tooling with `npm run r`, `npm run b`, target-specific builds, release sessions, and optional `gh` publication.
- **Codebase:** Added deterministic target-aware archives for Linux, macOS, and Windows, including both command binaries and GPL license text.
- **Testing:** Added scanner, cleaner, configuration, CLI parity, updater, checksum, release-session, and release-draft validation coverage.
- **Licenses:** Standardized project metadata and distributed documentation on GPL-3.0-or-later.
- **PKG:** Pinned Rust `1.98.1` and Node.js `24.20.0` release tooling.
- **PKG:** Updated dependencies (`dirs 7`, `dialoguer 0.12`, `sha2 0.11`, `toml 1.x`, `zip 8`).

## ℹ️ Release Info

- **GPG Signed:** Release archives, setup files, and target checksum manifests are signed with detached GPG signatures when published.
- **GPG Key:** Verify manifests against the pinned release key `CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F` at https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/release-signing-key.asc.
- **Code Signing:** macOS release binaries require hardened runtime signing. Windows binaries and setup files require Authenticode signing with RFC3161 timestamps.
- **Updater:** Installed binaries use verified HTTPS release metadata and refuse unverified replacements.
- **License:** GPL-3.0-or-later. See [`LICENSE`](LICENSE).

### This changelog is made using the BCLS standard: https://github.com/BurntToasters/BCLS
