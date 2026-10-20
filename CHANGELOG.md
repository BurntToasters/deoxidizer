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
> entry. `deox --update` verifies manifest signatures itself (no `gpg`
> needed). Windows binaries and setup files require Authenticode verification.
> Linux binaries are static (musl) and run on any distribution.
>
> This project is pre-1.0. Bugs, rough edges, and breaking changes are possible.

### ℹ️ Enjoying deoxidizer? Consider [❤️ Supporting Me! ❤️](https://rosie.run/support)

## Changes in `v0.1.0:`

- **NEW - Rust Artifact Cleaner:** Added safe scanning and cleanup for Rust and Tauri `target/` build artifacts.
- **NEW - Cleaning Modes:** Added `full`, `debug-only`, `incremental-only`, and `deps-only` cleanup modes.
- **NEW - Dual Invokers:** Added `deoxidizer` and `deox` commands with shared behavior and update logic.
- **NEW - Workspace Detection:** Added Tauri dependency detection for renamed dependencies, inherited workspace dependencies, and shared workspace targets. A workspace sharing one `target/` is shown once under the workspace's name.
- **NEW - Selective Cleanup:** Added `clean --select` (interactive picker), `clean --keep-bundles` (keep Tauri installers), and `--json` output for `scan` and `clean`.
- **NEW - Exact Estimates:** `clean --dry-run` and the confirmation prompt measure exactly what the clean removes.
- **NEW - Build Safety:** Projects with an active Cargo build are skipped instead of being deleted mid-build.
- **Security:** Reject symlinked project roots, targets, and intermediate paths, and revalidate target containment before cleanup. Symlinks inside `target/` are removed as links, never followed.
  - Scan, inspect, and dry-run operations remain read-only.
  - Clean operations require confirmation unless `--yes` is explicitly provided.
- **Updater:** Added HTTPS-only release downloads, bounded responses, safe archive extraction, and mandatory platform checksum verification.
- **Updater:** Authenticates checksum manifests in-process against pinned GPG key `CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F` before replacement; no external `gpg` required.
- **Updater:** Added `--check-update`, proxy support (`HTTPS_PROXY`), and GitHub's current release CDN host.
- **Codebase:** Added shared Node.js release tooling with `npm run r`, `npm run b`, target-specific builds, release sessions, and optional `gh` publication.
- **Codebase:** Added deterministic target-aware archives for Linux, macOS, and Windows, including both command binaries and GPL license text.
- **Testing:** Added an end-to-end suite that drives the real binary against realistic project trees and records a transcript per scenario, plus scanner, cleaner, configuration, CLI parity, updater, signature, checksum, release-session, and release-draft coverage.
- **Licenses:** Standardized project metadata and distributed documentation on GPL-3.0-or-later.
- **PKG:** Pinned Rust `1.98.1` and Node.js `24.20.0` release tooling.
- **PKG:** Updated dependencies (`dirs 7`, `dialoguer 0.12`, `sha2 0.11`, `toml 1.x`, `zip 9`, `rustls 0.23.46`).
- **PKG:** Cargo dependency updates that are semver-major are ignored by Dependabot; they go through the guarded `npm run update:safe` path with code changes.
- **Safety:** Only a `target/` holding Cargo's `CACHEDIR.TAG` or a regular `.rustc_info.json` is treated as a build directory. Other `target/` folders are reported as `not a Cargo build directory`, never deleted, and re-checked at clean time.
- **Scanning:** Plain folders named `target` that are not build outputs are scanned, so crates nested inside them are found.
- **Scripting:** `scan --json` and `clean --json` include a top-level `"schema_version": 1`; exit codes are `0` (success or cancelled), `1` (runtime error, partial clean, or analysis failures), and `2` (usage error). These follow semver from 1.0.
- **Output:** Project names, member names, and paths in human output replace control characters with U+FFFD, so names from cloned repositories cannot inject terminal escape sequences.
- **Settings:** Saving settings through a symlinked config file writes the link's target and keeps the symlink.
- **Units:** `--min-size-mb`, `--min-size`, and `min_size_mb` are documented and measured in MiB.
- **Updater:** `deox --update` replaces the sibling binary atomically (temp file, then rename).
- **Release:** `scripts/verify-release.cjs` verifies every `SHA256SUMS` manifest with the same in-process verifier `deox --update` uses; the release-key check also requires the pinned fingerprint in `src/updater.rs`.

## ℹ️ Release Info

- **GPG Signed:** Release archives, setup files, and target checksum manifests are signed with detached GPG signatures when published.
- **GPG Key:** Verify manifests against the pinned release key `CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F` at https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/release-signing-key.asc.
- **Code Signing:** macOS release binaries require hardened runtime signing. Windows binaries and setup files require Authenticode signing with RFC3161 timestamps.
- **Updater:** Installed binaries use verified HTTPS release metadata and refuse unverified replacements.
- **License:** GPL-3.0-or-later. See [`LICENSE`](LICENSE).

### This changelog is made using the BCLS standard: https://github.com/BurntToasters/BCLS
