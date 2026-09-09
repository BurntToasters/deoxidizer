# Deoxidizer Agent Guide

## Important

This file is the durable briefing for AI agents and developers working in this
repository. After making any architectural, configuration, or operational change
that future agents need in order to work correctly, update this file in the same
commit. Prefer concrete paths, command names, env vars, and fail-closed rules over
generalities.

All project code is licensed under the **GNU General Public License v3.0 or
later (GPL-3.0-or-later)**.

REUSE decision: `LICENSE` plus Cargo/package metadata only; no per-file license headers.

---

## Product Overview

**deoxidizer** is a fast, safe, disk space reclamation CLI tool specifically
tailored for Rust and Tauri development workspaces. Build directories (`target/`)
in Tauri and Rust apps grow aggressively (often 5–25+ GiB per project due to
unoptimized debug artifacts, compiler cache blobs, and dependency compilation units).

Deoxidizer scans configured project directories, identifies reclaimable build
artifacts, breaks down their disk usage (debug vs release, incremental caches,
compiled dependencies, and cross-compilation triples), and selectively cleans
them permanently or moves them to the OS Recycle Bin / Finder Trash.

### Dual Invoker Parity
The tool is packaged and installed with **dual command invokers**:
- `deoxidizer` (full name)
- `deox` (concise daily alias)

Both invokers must maintain **100% behavioral parity**. They share the identical
underlying library (`deoxidizer_lib`), CLI parser, configuration path, and
update logic.

---

## Hard Do-Nots (Invariants)

These rules are non-negotiable. Do not violate them for convenience.

1. **Never delete outside verified build artifact paths.** Only directories
   resolving directly under a valid `target/` subdirectory of a detected Rust/Tauri
   project may ever be removed. Never touch source code, `src/`, `src-tauri/src/`,
   `Cargo.toml`, `.git/`, or parent directories.
2. **Never follow symlinks when scanning or deleting.** Traversal must strictly
   use `.follow_links(false)`. Never allow symlink escapes to delete files outside
   the project tree.
3. **Never bypass interactive confirmation unless explicitly requested.**
   `deox clean` must always display the project count and reclaimable byte estimate
   and prompt the user `[Y/n]` before taking action, unless `--yes` / `-y` is
   explicitly provided.
4. **Never mutate the filesystem during scan, inspect, or dry-run.**
   `deox scan`, `deox inspect`, and `deox clean --dry-run` must remain strictly
   read-only operations.
5. **Never duplicate dispatch or execution logic between `main.rs` and `bin/deox.rs`.**
   Both `src/main.rs` and `src/bin/deox.rs` must remain minimal delegates calling
   `deoxidizer_lib::cli::run_app()`. All command routing, arguments, and execution
   belong in `src/cli.rs`.
6. **Never execute raw external shell commands (`rm`, `del`, `rmdir`) in Rust.**
   Filesystem operations must use `std::fs::remove_dir_all` (for permanent deletion)
   or the `trash` crate (for OS Trash / Recycle Bin).
7. **Never perform unverified self-updates.** The self-updater must strictly verify
   the SHA256 checksum of any downloaded asset against its platform-scoped
   `SHA256SUMS-<os>-<arch>.txt` manifest (or legacy global `SHA256SUMS.txt`)
   and verify detached manifest signature against checked-in
   `release-signing-key.asc` before invoking `self-replace`.
   Runtime updater hosts must provide `gpg`; absence or signature failure aborts update.
8. **Never use predictable fixed paths in `/tmp` for updates.** Updates must use
   `tempfile::Builder::new().prefix("deoxidizer-update-").tempdir()` to prevent
   symlink and pre-creation attacks on multi-user systems.
9. **Never introduce runtime network calls outside the updater.** Deoxidizer is a
   local-first tool. Do not add telemetry, usage metrics, analytics,
   crash-reporting pings, or remote phone-home features. Installed binaries may
   use network only when `deox --update` queries the GitHub Releases API over
   verified HTTPS. Explicitly invoked release scripts under `scripts/` may use
   `gh` for artifact publication; those scripts never run from normal CLI paths.
10. **Never commit secrets or credentials.** `.env` is gitignored. `.env.example`
    documents all release signing variables without real credentials.

---

## Repository State

- **GitHub Repository:** `BurntToasters/deoxidizer`
- **Current Version:** `0.1.0`
- **Target OS Support:**
  - macOS (Apple Silicon `aarch64` and Intel `x86_64`)
  - Linux (`x86_64` and `aarch64`)
  - Windows (`x86_64` and `aarch64`)
- **Default Installation Paths:**
  - macOS/Linux: `~/.local/bin/deoxidizer` (with `~/.local/bin/deox` symlink)
  - Windows: `%LOCALAPPDATA%\Programs\deoxidizer\deoxidizer.exe` and `deox.exe`

---

## Architecture & Codebase Layout

```text
deoxidizer/
├── Cargo.toml                  # Package manifest: lib + [[bin]] deoxidizer + [[bin]] deox
├── rust-toolchain.toml         # Pinned Rust toolchain and quality components
├── .node-version               # Pinned Node.js release-tool version
├── package.json                # Node release-tool commands
├── package-lock.json           # Locked Node release-tool metadata
├── .env.example                # Template for GPG, Apple, and Azure signing credentials
├── .gitignore                  # Git ignore rules (excludes /target, .env, release/, etc.)
├── README.md                   # User-facing documentation and usage guide
├── CHANGELOG.md                # BCLS-formatted release notes and GitHub body
├── AGENTS.md                   # This durable agent context and invariant specification
├── release-signing-key.asc     # Pinned public key for release manifest verification
├── install.sh                  # macOS/Linux installer script (local build or GitHub release)
├── install.ps1                 # Windows PowerShell installer script
├── installer.nsi               # Windows NSIS GUI installer script
├── .github/workflows/ci.yml    # Cross-platform quality gates
├── .github/workflows/release.yml # Manual target release workflow
├── .github/dependabot.yml      # Weekly Cargo/npm/action update checks
├── src/
│   ├── lib.rs                  # Library entrypoint; re-exports all modules
│   ├── main.rs                 # `deoxidizer` binary entrypoint (delegates to run_app)
│   ├── bin/
│   │   └── deox.rs             # `deox` binary entrypoint (delegates to run_app)
│   ├── cli.rs                  # Clap definitions, subcommands, args, and run_app dispatcher
│   ├── config.rs               # JSON config management at ~/.deox_config & safe ~ expansion
│   ├── project.rs              # Core models: DiscoveredProject, ProjectKind, TargetBreakdown
│   ├── scanner.rs              # Single-pass traversal & structural Cargo.toml/workspace parser
│   ├── cleaner.rs              # Clean modes, target triple handling, trash/delete executors
│   ├── display.rs              # Colored terminal output, scan tables, inspection view
│   ├── setup.rs                # Interactive setup wizard via dialoguer + --default
│   ├── settings.rs             # Settings management: show, config, reset
│   └── updater.rs              # GitHub API release checker, SHA256 validator, self-replace
├── scripts/
│   ├── build-release.sh        # Builds release binaries and packages tar.gz / zip
│   ├── gpg-sign.sh             # Generates target-scoped checksums and GPG signatures
│   ├── branch-sync.cjs         # Explicitly confirmed main/beta Git synchronization
│   ├── vi.cjs                  # Explicitly confirmed checkout bootstrap
│   ├── git-prune.cjs           # Explicitly confirmed local-only branch deletion
│   ├── sync-version.cjs        # Version synchronization across manifests and changelog
│   ├── release.cjs             # Target build, signing, and optional publication
│   ├── release-session.cjs     # Quality proof and release identity binding
│   ├── github-cli.cjs          # Token-scrubbed gh wrapper
│   ├── ensure-draft-release.cjs # Draft release create/wait coordination
│   ├── verify-release.cjs      # Local and remote artifact verification
│   ├── verify-release-draft.cjs # Required multi-target draft completeness
│   ├── macos-codesign.sh       # Apple Developer ID signing + runtime hardening + notarytool
│   ├── windows-artifact-sign.ps1  # Azure Artifact Signing with RFC3161 timestamps
│   ├── verify-windows-authenticode.ps1 # Validates Authenticode signature on .exe files
│   ├── artifact-signing-tools.ps1      # Tool locator for signtool.exe and Azure dlib
│   ├── setup-windows-artifact-signing.ps1 # VM setup helper for Azure Artifact Signing tools
│   ├── publish-release.cjs      # Explicit draft-to-published transition
│   ├── check-license.cjs        # GPL metadata consistency check
│   ├── check-changelog.cjs      # BCLS release metadata and asset-link check
│   ├── check-release-key.cjs    # Pinned GPG release key verification
│   ├── check-toolchain.cjs      # Rust/Node toolchain pin verification
│   └── check-version.cjs        # Cargo/package version consistency check
└── tests/
    ├── cleaner_test.rs         # Unit and integration tests for clean modes and path safety
    ├── cli_test.rs             # Dual-binary parity and CLI rejection tests
    ├── config_test.rs          # Serialization, validation, and safe tilde expansion tests
    ├── scanner_test.rs         # Mock projects, workspaces, Tauri detection, and ignore tests
    ├── node/release-tools.test.cjs # Node release helper tests
    └── node/sync-version.test.cjs  # Node version-sync helper tests
```

---

## Configuration Specification (`~/.deox_config`)

The configuration file is stored in the user's home directory as JSON:
`~/.deox_config`

### JSON Schema
```json
{
  "version": 2,
  "projects_dir": "/Users/dev/Documents/GitHub",
  "scope": "tauri-only",
  "clean_behavior": "delete",
  "default_mode": "full",
  "min_size_mb": 0,
  "ignored_projects": []
}
```

### Fields and Types
- `version` (`u32`): Schema version for future migrations (currently `2`). Version `1` files migrate on load.
- `projects_dir` (`String`): Root directory scanned for projects. Supports `~/` expansion.
- `scope` (`Scope`):
  - `"tauri-only"` (default): Only scans projects with Tauri dependencies.
  - `"tauri-and-rust"`: Scans both Tauri apps and standard Rust crates/workspaces.
  - `"rust-only"`: Only scans plain Rust projects (excluding Tauri).
- `clean_behavior` (`CleanBehavior`):
  - `"delete"` (default): Permanently removes target directories via filesystem deletion.
  - `"trash"`: Moves target directories to the OS Recycle Bin / Trash using the `trash` crate.
- `default_mode` (`DefaultMode`):
  - `"full"` (default): Removes the entire `target/` directory.
  - `"debug-only"`: Removes `debug/` directories while preserving `release/`.
  - `"incremental-only"`: Removes incremental compiler cache directories (`*/incremental`).
  - `"deps-only"`: Removes dependency compilation units (`*/deps`).
- `min_size_mb` (`u64`): Minimum artifact size in megabytes to include in scan/clean (0 = all).
- `ignored_projects` (`Vec<String>`): Project names to always exclude from scan and clean operations.

---

## Cleaning Modes & Target Breakdown

Cargo target directories contain distinct artifacts with different recreation costs:

Rough heuristics, not guarantees:

| Clean Mode | Target Paths Removed | Reclaimable Space | Rebuild Cost | Typical Use Case |
|---|---|---|---|---|
| `full` | Entire `target/` directory | 100% | High (recompiles all dependencies) | Finished projects, archive, maximum space recovery |
| `debug-only` | `target/debug/` and `target/<triple>/debug/` | ~80–90% | Moderate (keeps optimized release builds) | Developer machines with precious release binaries |
| `incremental-only` | `*/incremental/` across all profiles (depth ≤ 3) | ~15–25% | Low (keeps compiled `.rlib`/`.o` objects) | Routine maintenance during active daily development |
| `deps-only` | `*/deps/` across all profiles (depth ≤ 3) | ~70–80% | Moderate to High | Clears stale dependencies while keeping target root |

### Cross-Compilation Target Triples
Tauri apps frequently compile for cross-platform targets (e.g. `target/x86_64-pc-windows-msvc/debug`
or `target/aarch64-apple-darwin/release`).
`scanner.rs` and `cleaner.rs` must both recognize and process target triple directories in
addition to top-level `debug/` and `release/`.
`debug-only` treats any non-`debug`/non-`release` subdirectory of `target/`
as a triple candidate and cleans its `debug/` child when present; do not narrow
to a known triple list. `incremental-only`/`deps-only` collect matches up to
depth 3 and intentionally under-clean deeper layouts fail-closed.
`trash` behavior uses the OS Trash (Finder Trash, Recycle Bin, freedesktop
Trash); trashed files still occupy disk until emptied, and `trash failed` does
not fall back to `delete`. Sizes are logical file bytes and `clean --dry-run`
is the canonical reclaimable-byte source; breakdown subtotals overlap, so do
not sum them.

---

## Binary Signing & Release Pipeline (Zinnia-Compatible)

The release and signing architecture follows the identical environment framework as Zinnia:

### Environment Variables (`.env`)
```bash
# Node release orchestration
GH_REPO_OWNER=BurntToasters
GH_REPO_NAME=deoxidizer
DEOX_RELEASE_CONFIRM=
DEOX_RELEASE_DRAFT_MODE=create
DEOX_ALLOW_UNSIGNED_RELEASE=0
DEOX_CHECKSUM_NAME=
# Internal/advanced: set by release tooling to bind the release session target
# (e.g. host); do not set manually.
DEOX_RELEASE_TARGET=

# GPG Signing
GPG_KEY_ID=
GPG_PASSPHRASE=

# Windows Azure Artifact Signing (Authenticode)
AZURE_CLIENT_ID=
AZURE_TENANT_ID=
AZURE_SUBSCRIPTION_ID=
AZURE_CLIENT_SECRET=
AZURE_ARTIFACT_SIGNING_ENDPOINT=
AZURE_ARTIFACT_SIGNING_ACCOUNT=
AZURE_ARTIFACT_SIGNING_PROFILE=
AZURE_ARTIFACT_SIGNING_PUBLISHER=
AZURE_ARTIFACT_SIGNING_PUBLISHER_DN=
SKIP_WIN_CODESIGN=0

# macOS Codesigning & Notarization
APPLE_SIGNING_IDENTITY=
APPLE_ID=
APPLE_PASSWORD=
APPLE_TEAM_ID=
APPLE_KEYCHAIN_PROFILE=
```

### Platform Signing Rules
1. **Windows Authenticode:**
   - Run on Windows release VMs via `scripts/windows-artifact-sign.ps1`.
   - Uses `signtool.exe` with `Azure.CodeSigning.Dlib.dll` and RFC3161 timestamping.
   - Enforces subject DN match against `$env:AZURE_ARTIFACT_SIGNING_PUBLISHER_DN`.
   - NSIS setup output is signed before packaging and loose/archive contents are
     verified using `scripts/verify-windows-authenticode.ps1`.
2. **macOS Codesigning:**
   - Run via `scripts/macos-codesign.sh`.
   - Developer ID signing requires `--options runtime --timestamp`.
   - Ad-hoc signing (`-s -`) requires explicit `--allow-adhoc` and is for local
     staging only; release workflows fail when identity is missing.
   - Submits `.zip` payload to `xcrun notarytool` when `APPLE_KEYCHAIN_PROFILE` is set.
   - Notarization uses preconfigured `APPLE_KEYCHAIN_PROFILE`; passwords never enter process arguments.
3. **Artifact Integrity & GPG:**
   - Run via `scripts/gpg-sign.sh release`.
   - Generates `SHA256SUMS-<os>-<arch>.txt` for target archives and setup assets.
   - Requires `GPG_KEY_ID` unless `--allow-unsigned` is explicitly used for local staging.
   - Passphrases enter through stdin, never command-line arguments.
4. **Node release orchestration:**
   - `npm run r` and `npm run b` require `DEOX_RELEASE_CONFIRM=YES` because they reset and clean Git state.
  - `npm run u -- <version>` synchronizes `package.json`, `package-lock.json`, `Cargo.toml`, `Cargo.lock`, and BCLS download metadata.
    - `npm run release:windows[:arch]` creates the single draft and uploads Windows artifacts.
       `npm run release:linux[:arch]` and `npm run release:macos[:arch]` wait for that
       draft and upload their artifacts. Release sessions require a clean Git checkout
       so artifacts match bound `HEAD`.
    - `npm run release:upload` remains an explicit staged-artifact upload path after
       `gh auth login`; publication uses draft releases and remote digest checks.
       GitHub Actions runs quality checks only; production release builds run manually
       on dedicated release VMs.
   - Target builders may upload incrementally to the same draft; remote verification
      permits only known release asset names and matches each local upload by digest.
      Final publication re-verifies every remote platform checksum manifest against
      the pinned signing key and GitHub asset digests.
  - Draft releases bind `target_commitish` to current Git `HEAD`; existing drafts with
    a different binding are rejected. Release archive verification lists ZIP entries
    in Node without requiring an external `unzip` command.
  - Manual release workflow OS choices use `linux`, `macos`, and `windows`; `macos`
    maps to the package scripts while Rust target triples use `darwin`.
  - `CHANGELOG.md` is the BCLS-formatted release body; draft creation fails if it is missing or empty.

---

## Testing & Quality Gates

Before committing or submitting changes, verify that the repository passes all
quality gates:

```bash
# 1. Formatting check (must produce zero diffs)
cargo fmt --check

# 2. Clippy linter (must pass with zero warnings)
cargo clippy --all-targets -- -D warnings

# 3. Unit and integration tests
cargo test

# 4. Node release-tool tests, license, version, and changelog checks
npm ci
npm install --global npm@12.0.2  # CI pins npm major required by package.json
npm run check:license
npm run check:version
npm run check:changelog
npm run check:toolchain
npm run check:release-key
npm run quality:node
for f in install.sh scripts/*.sh; do bash -n "$f"; done

# 5. Release build verification
cargo build --release --locked

# 6. Dual binary verification
./target/release/deoxidizer --version
./target/release/deox --version
```

`cargo run -- --help` fails with two binaries; use
`cargo run --bin deox -- --help` for ground-truth help output.

### Test Coverage Highlights
- `tests/config_test.rs`: Validates default configuration, atomic round-trip serialization, malformed/future-version rejection, enum parsing, and safe tilde expansion.
- `tests/scanner_test.rs`: Validates Tauri project detection, renamed/inherited dependencies, shared workspaces, size calculation, symlink exclusion, and `ignored_projects` filtering.
- `tests/cleaner_test.rs`: Validates deletion in all 4 clean modes, target triples, symlink rejection, dry-run safety, and preservation of release builds in `debug-only` mode.
- `tests/cli_test.rs`: Validates dual-binary version/output parity and invalid-mode rejection.
- `tests/node/release-tools.test.cjs`: Validates target mapping, checksums, release identity, token scrubbing, and destructive Git confirmation.
- `tests/node/sync-version.test.cjs`: Validates version parsing, Cargo/npm manifest updates, and BCLS changelog rewrites.
