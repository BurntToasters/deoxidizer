# Deoxidizer Agent Guide

# Critical Information - Do not modify

- NEVER write unit tests after you write code.
- Highly prefer E2E tests as the sole testing mechanism. Use them to verify complex features work. At the end of E2E tests, produce a verifiable and repeatable artifact.
- If you must test a system in isolation, FIRST write all the ways it could fail, THEN write the code.


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
them permanently or, on Linux, moves them to the freedesktop Trash through
verified same-filesystem staging. Trash mode currently fails without mutation
on macOS and Windows until a safe native backend is available.

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
   A `target/` folder counts as a Cargo build directory only when it holds
   Cargo's `CACHEDIR.TAG` (contents start with
   `Signature: 8a477f597d28d172789f06886806bc55`) or a regular, non-symlink
   `.rustc_info.json`. Any other `target/` is warned about as
   `not a Cargo build directory`, is never deleted, and is re-checked at clean
   time (`scanner::is_cargo_target_dir`, `cleaner::validate_target_path`).
   Project roots may be on separate filesystems from their parents. On Unix,
   clean planning refuses a mounted `target/` or mounted descendants. Linux
   uses `openat2(RESOLVE_NO_XDEV)`, which also catches same-device bind mounts;
   when unavailable or blocked, planning fails closed with an unsupported error.
   Other Unix systems compare device IDs, so same-device bind detection is
   Linux-specific. Windows retains handle-anchored path validation; this Unix
   mount-boundary check does not apply there.
2. **Never follow symlinks when scanning or deleting.** Traversal must strictly
   use `.follow_links(false)`. Never allow symlink escapes to delete files outside
   the project tree. A symlinked project root, `target/`, or intermediate
   directory on a clean path is refused; symlinks *inside* a clean path are
   removed as links (`remove_dir_all`/`remove_file` never follow them). Do not
   reintroduce a blanket "refuse any symlink inside target" rule: Tauri AppImage
   bundles and CMake-built `-sys` crates create such links routinely.
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
   Permanent filesystem operations use handle-anchored deletion. Linux Trash
   uses verified same-filesystem staging; macOS and Windows Trash requests fail
   before mutation until a safe native backend exists.
7. **Never perform unverified self-updates.** The self-updater must verify the
   SHA256 checksum of the downloaded asset against its platform-scoped
   `SHA256SUMS-<os>-<arch>.txt` manifest (or legacy global `SHA256SUMS.txt`)
   and verify the raw `.minisig` manifest signature against the checked-in
   `release-signing-key.pub` before replacement. `src/minisign.rs` pins the
   exact Ed25519 public-key serialization and verifies signatures in-process;
   no external Minisign or GPG process runs in the updater. Any parse or
   verification failure aborts the update. Do not widen the accepted Minisign
   profile or enable legacy Ed signatures without failure-mode tests first.
   Keep the signing private key outside the checkout; key changes must update
   the runtime, installers, release checker, and documented key ID together.
   `deox --update` replaces the sibling binary atomically: it writes a
   `.deox-sibling-*` temp file in the destination directory, then renames it
   into place. `scripts/verify-release.cjs` runs the same in-process verifier
   at release time (see the release pipeline section below).
8. **Never use predictable fixed paths in `/tmp` for updates.** Updates must use
   `tempfile::Builder::new().prefix("deoxidizer-update-").tempdir()` to prevent
   symlink and pre-creation attacks on multi-user systems.
9. **Never introduce runtime network calls outside the updater.** Deoxidizer is a
   local-first tool. Do not add telemetry, usage metrics, analytics,
   crash-reporting pings, or remote phone-home features. Installed binaries may
   use network only for `deox --update` and `deox --check-update`, which query
   `https://api.github.com/repos/BurntToasters/deoxidizer/releases/latest` over
   verified HTTPS. `--check-update` stops after version comparison;
   `--update` may download the selected archive, checksum manifest, and
   `.minisig` sidecar from GitHub's release asset hosts. Explicitly invoked
   release scripts under `scripts/` may use `gh`; those scripts never run from
   normal CLI paths.
10. **Never commit secrets or credentials.** `.env` is gitignored. `.env.example`
    documents all release signing variables without real credentials.

---

## Repository State

- **GitHub Repository:** `BurntToasters/deoxidizer`
- **Current Version:** `0.1.0`
- **Target OS Support:**
  - macOS (Apple Silicon `aarch64` and Intel `x86_64`)
  - Linux (`x86_64` and `aarch64`, static musl targets; distro coverage is not exhaustive)
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
├── .env.example                # Template for Tauri Minisign, Apple, and Azure signing credentials
├── .gitignore                  # Git ignore rules (excludes /target, .env, release/, etc.)
├── README.md                   # User-facing documentation and usage guide
├── CHANGELOG.md                # BCLS-formatted release notes and GitHub body
├── AGENTS.md                   # This durable agent context and invariant specification
├── release-signing-key.pub     # Pinned Minisign public key for release verification
├── deny.toml                   # cargo-deny advisories/licenses/sources policy
├── .gitattributes              # Keeps signed test fixtures byte-exact (-text)
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
│   ├── updater.rs              # GitHub API release checker, SHA256 validator, self-replace
│   └── minisign.rs             # In-process verifier for pinned Minisign signatures
├── scripts/
│   ├── build-release.sh        # Builds release binaries and packages tar.gz / zip
│   ├── minisign-sign.cjs       # Generates checksums and raw .minisig sidecars via Tauri CLI
│   ├── branch-sync.cjs         # Explicitly confirmed main/beta Git synchronization
│   ├── vi.cjs                  # Explicitly confirmed checkout bootstrap
│   ├── git-prune.cjs           # Explicitly confirmed local-only branch deletion
│   ├── sync-version.cjs        # Version synchronization across manifests and changelog
│   ├── npm-safe-update.cjs     # Age-gated npm lock updates (3d min-release-age, lock-only, audited)
│   ├── cargo-safe-update.cjs   # Age-gated cargo updates (72h min publish age, validated temp lockfile)
│   ├── check-cargo-update-policy.cjs # Regression scanner: dep mutations must route via safe updaters
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
│   ├── check-release-key.cjs    # Pinned Minisign release key verification
│   ├── check-toolchain.cjs      # Rust/Node toolchain pin verification
│   └── check-version.cjs        # Cargo/package version consistency check
├── examples/
│   ├── verify_release_archive.rs # Release-time archive and embedded-proof verifier
│   ├── verify_release_manifest.rs # In-process Minisign manifest verifier
│   └── verify_release_signature.rs # In-process Minisign sidecar verifier
└── tests/
    ├── e2e_test.rs             # E2E: fixture trees + real `deox` binary; transcripts in target/e2e-artifacts/
    ├── cleaner_test.rs         # Unit and integration tests for clean modes and path safety
    ├── cli_test.rs             # Dual-binary parity and CLI rejection tests
    ├── config_test.rs          # Serialization, validation, and safe tilde expansion tests
    ├── scanner_test.rs         # Mock projects, workspaces, Tauri detection, and ignore tests
    ├── node/release-tools.test.cjs # Node release helper tests
    ├── node/sync-version.test.cjs  # Node version-sync helper tests
    ├── node/cargo-safe-update.test.cjs # Safe-updater age-gate, lockfile transaction, env tests
    └── node/check-cargo-update-policy.test.cjs # Update-policy scanner regression tests
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
  - `"trash"`: On Linux, stages the verified target on the same filesystem before using freedesktop Trash. On macOS and Windows, the request returns a no-mutation error until a native identity-safe backend exists.
- `default_mode` (`DefaultMode`):
  - `"full"` (default): Removes the entire `target/` directory.
  - `"debug-only"`: Removes `debug/` directories while preserving `release/`.
  - `"incremental-only"`: Removes incremental compiler cache directories (`*/incremental`).
  - `"deps-only"`: Removes dependency compilation units (`*/deps`).
- `min_size_mb` (`u64`): Minimum artifact size in mebibytes (MiB) to include in scan/clean (0 = all). The `--min-size-mb` / `--min-size` flags use the same unit.
- `ignored_projects` (`Vec<String>`): Names to always exclude from scan and clean. A name matches a
  project's name or any package in its `members` list (case-insensitive), so ignoring one
  workspace member protects the shared `target/`.

`projects_dir` is stored absolute (`config::normalize_projects_dir`); a leading `~` is kept
verbatim and expanded at use (`config::expand_tilde`, also `~\` on Windows). Underscore
spellings (`tauri_only`) are rejected everywhere; there is no loose parser.

Saving settings through a symlinked `~/.deox_config` writes the link's target
and keeps the symlink in place.

### Scripting contract (`--json` and exit codes)
- `scan --json` and `clean --json` emit a top-level `"schema_version": 1`.
  Paths in JSON are strings decoded as lossy UTF-8.
- Exit codes: `0` = success or user cancelled; `1` = runtime error, partial
  clean, or `analysis_failures`; `2` = usage error (invalid flag, mode, scope,
  or value).
- From 1.0 these, the JSON schema version, and documented flags are covered by
  semver. Changing them requires updating README "Scripting and stability".

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
`trash` behavior currently works only on Linux. It stages the verified artifact
inside a private directory under the project, requires the stage and source to
share a filesystem, checks the staged file identity, and restores the source
when the Trash operation fails. A different-filesystem stage fails before
moving the artifact. macOS and Windows return a no-mutation error because their
native Trash APIs are not yet bound to the verified file identity. Trash
failures never fall back to `delete`; permanent deletion remains available on
all supported OSes. Sizes are logical bytes of regular files (symlinks count
as 0).

Future macOS research candidate: `FileManager.trashItem` with a
`fileReferenceURL`. Future Windows candidates: `IFileOperation` with
`FOFX_RECYCLEONDELETE` and its per-item placement callback. These are not
release prototypes or permission to change the current no-mutation behavior.
Any backend must prove identity despite path races, restore the original item
after every failed move, and fail closed without mutation when identity,
callback, or restoration cannot be proven.

### Project model and clean pipeline
- One `DiscoveredProject` per `target/` directory. `scanner::scan_dir_report`
  groups every package by the target it builds into; the project is named
  after the owning root (root `package.name`, else directory name) and lists
  all package names in `members`. Kind is Tauri if any member is Tauri.
- Target resolution: a real `target/` next to the manifest wins, else the
  *nearest* ancestor `[workspace]` (bounded by the scan root) owns the build;
  if it has no `target/`, the package has none. Virtual workspace roots are
  candidates themselves.
- Unreadable folders below the scan root are skipped with a warning (the root
  itself must be readable). Projects whose `target/` cannot be analyzed are
  counted in `ScanReport::analysis_failures`; `clean` exits 1 if any exist.
- `scanner::is_artifact_dir` skips `node_modules` directories and `target`
  directories that are build outputs (a `CACHEDIR.TAG` is present, or the
  parent holds a `Cargo.toml`). A plain folder named `target` (no
  `CACHEDIR.TAG`, no `Cargo.toml` in its parent) is descended into, so crates
  nested inside it are found.
- Only Cargo build directories become projects: `scanner::is_cargo_target_dir`
  (see Hard Do-Nots #1). Other `target/` folders produce a
  `not a Cargo build directory` warning and no project; `cleaner::plan` and
  `clean_project_with` call `cleaner::validate_target_path` again before any
  removal. The markers must survive partial cleans: `--keep-bundles` preserves
  `CACHEDIR.TAG`/`.rustc_info.json` alongside the bundles, and deleting a whole
  `target/` removes every other child first (`cleaner::remove_markers_last`),
  so an interrupted or failed clean can be finished by re-running it.
- Human output sanitizes project names, member names, and paths: control
  characters are replaced with U+FFFD so names from cloned repositories cannot
  inject terminal escape sequences. JSON output carries the raw names.
- `cleaner::plan` validates the root and target once, builds the path list
  for the mode (plus `--keep-bundles`, which preserves `*/release/bundle`),
  drops nested duplicates, and validates each path. `estimate_with` and
  `clean_project_with` both execute that plan, so `--dry-run` and the
  confirmation estimate equal the freed bytes. `TargetBreakdown` buckets use
  the same rules (profile at depth 1 or under a triple; `incremental`/`deps`
  at depth 1-3).
- `cleaner::build_in_progress` probes `.cargo-lock` files (depth <= 3) with a
  non-blocking `File::try_lock` on a read-only handle; a held lock skips the
  project as `build in progress`.

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

# Tauri CLI Minisign signing (must match key ID 083B254A48C428E3; private key stays outside repo)
TAURI_SIGNING_PRIVATE_KEY_PATH=
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=

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
   - `installer.nsi` edits the user PATH only after a successful, complete
     `ReadRegStr` (standard NSIS truncates at 1024 chars); otherwise it tells
     the user to add the directory manually. It needs `LogicLib.nsh` and
     `WordFunc.nsh`; CI compiles it with `makensis`.
   - PowerShell scripts must stay ASCII-only (PowerShell 5.1 misreads BOM-less
     UTF-8). `install.ps1` runs inside `& { }` so strict mode never leaks into
     the caller's session, writes PATH as `REG_EXPAND_SZ` via the registry,
     requires external `minisign.exe` for manifest verification, and requires
     Authenticode on both installed binaries.
2. **macOS Codesigning:**
   - Run via `scripts/macos-codesign.sh`.
   - Developer ID signing requires `--options runtime --timestamp`.
   - Ad-hoc signing (`-s -`) requires explicit `--allow-adhoc` and is for local
     staging only; release workflows fail when identity is missing.
   - Notarization is required: each binary is zipped and submitted to
     `xcrun notarytool --wait --output-format json`, and the parsed status
     must be `Accepted` (notarytool can exit 0 on rejection). Missing
     `APPLE_KEYCHAIN_PROFILE` fails unless `DEOX_ALLOW_UNNOTARIZED=1` and
     `DEOX_RELEASE_CONFIRM=YES` (local staging only).
   - Notarization uses preconfigured `APPLE_KEYCHAIN_PROFILE`; passwords never enter process arguments.
3. **Artifact Integrity & Minisign:**
   - Run via `scripts/minisign-sign.cjs` (`npm run release:sign`).
   - Generates `SHA256SUMS-<os>-<arch>.txt` and raw `.minisig` sidecars for each
     archive, Windows setup asset, and checksum manifest. The Tauri CLI emits an
     encoded `.sig` wrapper; this script validates and writes the raw sidecar.
   - Requires `TAURI_SIGNING_PRIVATE_KEY_PATH` on the provisioned signer; this
     key must pair with the checked-in key ID. The Tauri CLI reads
     `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from the environment. Do not generate
     a replacement key without updating every public-key pin.
   - The checked-in `release-signing-key.pub` is pinned as key ID
     `083B254A48C428E3` by `src/minisign.rs`, `install.sh`, `install.ps1`, and
     `scripts/check-release-key.cjs`. `deox --update` and release verification
     use the in-process verifier. First-install shell and PowerShell bootstraps
     require the external Minisign CLI. GPG is not used.
4. **Node release orchestration:**
   - Linux release targets are static `*-unknown-linux-musl` builds for x86_64
     and aarch64. `release.cjs` requires a matching native Linux host and
     `musl-gcc` (`musl-tools`); do not claim every Linux distribution has been
     tested.
   - `npm run r` and `npm run b` require `DEOX_RELEASE_CONFIRM=YES`. They fetch
     and verify `origin/main` or `origin/beta` before the first reset/clean.
     `--force-always` bypasses only the confirmation gate. `scripts/vi.cjs`
     verifies its upstream against the remote before resetting or cleaning.
     Both scripts invoke npm through Node and `npm_execpath` on Windows without
     a shell. `npm run r` also runs `scripts/git-prune.cjs`, which deletes only
     local branches whose upstream is `[gone]`.
   - `npm run u -- <version>` runs the safe npm and Cargo dependency updaters
     before syncing versions. npm uses a 3-day release-age gate, lock-only
     updates, and a high-severity audit; Cargo uses a 72-hour publish-age gate,
     blocks Git/foreign-registry dependencies, and installs a validated
     temporary lockfile. Bare `npm run u` propagates the `package.json` version
     across manifests and the BCLS download table. `npm run update:safe` only
     updates dependency locks. Never use raw `cargo update`, `npm update`, or
     `npm audit fix`; `npm run check:update-policy` fails closed on unguarded
     dependency mutation paths.
   - The six native release commands are `npm run release:windows:x64`,
     `npm run release:windows:arm64`, `npm run release:linux:x64`,
     `npm run release:linux:arm64`, `npm run release:macos:x64`, and
     `npm run release:macos:arm64`. Run each on its matching native OS and
     architecture VM from a clean checkout of the same candidate tag/commit.
     These are the one-step build/upload path. For stable v0.1.0, use the
     stage-first, serial-upload sequence below. The first Windows x86_64 upload
     creates the draft; subsequent uploads reuse it. Release sessions bind
     builds to `HEAD`; the remote tag must resolve to that same commit.
   - `npm run release:verify:draft -- --integrity` requires 28 assets: six archives, six
     platform checksum manifests, two Windows setup assets, and 14 `.minisig`
     sidecars. It checks draft identity and remote tag commit. Run
     `npm run release:verify:remote` to verify signatures, manifests, archive
     proofs, and GitHub asset digests. `npm run release:publish -- --yes`
     repeats remote verification before the irreversible draft-to-public
     transition and requires `DEOX_RELEASE_CONFIRM=YES`.
   - `npm run release:upload` is the staged-artifact path and requires `gh auth
     login`. GitHub Actions runs quality checks; production builds run on the
     six dedicated native VMs. Manual workflow OS choices are `linux`, `macos`,
     and `windows`; Rust uses `darwin` for macOS target triples.
   - macOS builds require a Developer ID certificate and accepted notarization
     using `APPLE_KEYCHAIN_PROFILE`; GitHub-hosted macOS runners do not have the
     required credentials. Windows builds need Azure Artifact Signing tools
     and credentials. `.github/workflows/release.yml` rejects hosted macOS
     runners for production signing.
   - `scripts/verify-release.cjs` verifies checksum-manifest signatures and
     payload `.minisig` sidecars with the in-process verifier used by
     `deox --update`, via the `verify_release_manifest` and
     `verify_release_signature` examples. It also validates archives through
     `verify_release_archive`. `scripts/check-release-key.cjs` pins the exact
     public-key serialization across runtime and installers.
   - `CHANGELOG.md` is the BCLS-formatted release body; draft creation fails if
     it is missing or empty.

### Stable v0.1.0 first-release runbook

1. Freeze a clean candidate commit for `v0.1.0`. Require all these gates on the
   exact SHA before signing or building:

   ```bash
   npm ci
   npm run quality
   cargo deny check advisories bans licenses sources
   ```

   `.github/workflows/ci.yml` must be a required check and green on that exact
   commit.

2. Before release builds, run the signing-key preflight on a scratch payload
   outside the checkout; it exercises Tauri signing and then the repository-
   pinned in-process verifier:

   ```bash
   scratch="$(mktemp)"
   trap 'rm -f "$scratch" "$scratch.minisig"' EXIT
   printf 'deoxidizer v0.1.0 signing preflight\n' > "$scratch"
   node -e 'const s=require("./scripts/minisign-sign.cjs");
   s.signFile(process.argv[1],s.minisignEnvironment(process.env));' "$scratch"
   ```

   Require key ID `083B254A48C428E3`; private-key material stays outside the
   checkout and passwords never enter command arguments.

3. Before any upload, check the actual remote `v0.1.0` tag and release for a
   collision. Stop if either exists or points elsewhere. Create and push an
   annotated tag at the frozen candidate SHA, resolve the actual remote tag,
   and require its peeled commit to equal `git rev-parse HEAD`. Do not infer tag
   binding from GitHub `target_commitish`.

4. On matching native OS/architecture VMs, stage all six signed targets without
   `--upload`:

   ```bash
   node scripts/release.cjs windows x86_64
   node scripts/release.cjs windows aarch64
   node scripts/release.cjs linux x86_64
   node scripts/release.cjs linux aarch64
   node scripts/release.cjs darwin x86_64
   node scripts/release.cjs darwin aarch64
   ```

   Wait for the fixture checks in `npm run quality` and local verification of
   all six stages before the first upload. Each target lives under
   `release/staging/<rust-target>`; rerun its verifier by setting
   `DEOX_RELEASE_STAGE_DIR` to that path and running `npm run release:verify`.
   Use native Developer ID/notarization on macOS, verified Azure Authenticode
   on Windows, and matching Linux VMs with `musl-gcc`.

   Before any upload, run the offline signed candidate rehearsal on native
   Windows x86_64 for a complete pass; retain its transcript. On every native
   OS/architecture VM, install/extract the exact staged candidate (not the
   public latest-release bootstrap) into a disposable prefix, verify both
   invokers, and capture `deox clean --dry-run` plus a confirmed
   cleanup against a disposable marked Cargo fixture and isolated config.
   Prove Linux Trash placement, macOS/Windows Trash refusal before mutation,
   and permanent delete on all supported OSes. On a clean Windows account
   with a short user PATH, install the staged NSIS setup and verify the user
   PATH and both invokers;
   uninstall it and verify its app files and PATH entry are removed. Store
   transcripts, fixture hashes, and Windows PATH/uninstall evidence under
   `target/e2e-artifacts/`. Do not upload until all evidence is complete.

   ```bash
   node scripts/rehearse-signed-update.cjs --stage-dir release/staging/x86_64-pc-windows-msvc --binary-dir target/x86_64-pc-windows-msvc/release --output target/e2e-artifacts/signed-update-rehearsal-windows-x64.json
   ```

5. Upload one target at a time from its native VM after `gh auth login`.
   Windows x86_64 creates the draft; use this order and do not overlap uploaders:

   ```bash
   npm run release:upload -- --target x86_64-pc-windows-msvc
   npm run release:upload -- --target aarch64-pc-windows-msvc --wait
   npm run release:upload -- --target x86_64-unknown-linux-musl --wait
   npm run release:upload -- --target aarch64-unknown-linux-musl --wait
   npm run release:upload -- --target x86_64-apple-darwin --wait
   npm run release:upload -- --target aarch64-apple-darwin --wait
   ```

6. After all uploads, require
   `npm run release:verify:draft -- --integrity` to pass: exactly 28 assets
   (six archives, six platform checksum manifests, two Windows installers, and
   14 `.minisig` sidecars), correct remote tag commit, signatures, and all six
   native proofs. The release owner must explicitly approve before
   `DEOX_RELEASE_CONFIRM=YES npm run release:publish -- --yes`.

7. After publishing v0.1.0, perform a fresh public discovery check only with
   `deox --check-update`; record the host and output. At the next stable
   release, after that candidate is public, use a disposable v0.1.0 installation
   for the first real GitHub upgrade:

   ```bash
   deox --check-update
   deox --update
   deox --version
   deoxidizer --version
   ```

   Record the OS/architecture, output, and before/after versions under
   `target/e2e-artifacts/`. This public updater proof is separate from the
   offline signed candidate rehearsal in step 4.

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
for f in install.sh scripts/*.sh; do bash -n "$f"; done   # never `bash -n a b`: it only checks `a`

# 5. Supply chain
cargo deny check advisories bans licenses sources

# 6. Release build verification
cargo build --release --locked

# 7. Dual binary verification
./target/release/deoxidizer --version
./target/release/deox --version
```

`cargo run -- --help` fails with two binaries; use
`cargo run --bin deox -- --help` for ground-truth help output.

CI additionally parse-checks and lints every PowerShell script
(PSScriptAnalyzer errors fail), compiles `installer.nsi` with `makensis`,
builds static musl binaries natively on x86_64 and arm64 runners, and uploads
`target/e2e-artifacts/` (E2E transcripts) from every OS. Dependabot uses a
3-day cooldown matching the safe-updater age gates. Dependabot ignores
semver-major Cargo updates (`.github/dependabot.yml`): a major `zip` bump broke
`main`, so majors go through `npm run update:safe` with the code changes.

CI (`.github/workflows/ci.yml`) must be a required status check on `main`
(branch protection). `main` stayed red for weeks while it was not enforced.

### Test Coverage Highlights
- `tests/e2e_test.rs`: Drives the real `deox` binary against fixture trees (Tauri AppImage symlinks, unreadable folders, workspaces, nested paths, build locks, bundles) and writes a transcript per scenario to `target/e2e-artifacts/` (override with `DEOX_E2E_ARTIFACTS`). Permission scenarios skip when run as root; run them as an unprivileged user to exercise them.
- `src/minisign.rs` tests: Verify the upstream non-legacy Minisign vector, reject tampering and legacy Ed signatures, reject malformed/extra-line data, and require the exact checked-in pinned key. Node release tests cover normalization of Tauri CLI signatures into raw `.minisig` sidecars.
- `tests/config_test.rs`: Validates default configuration, atomic round-trip serialization, malformed/future-version rejection, enum parsing, and safe tilde expansion.
- `tests/scanner_test.rs`: Validates Tauri project detection, renamed/inherited dependencies, shared workspaces, size calculation, symlink exclusion, and `ignored_projects` filtering.
- `tests/cleaner_test.rs`: Validates deletion in all 4 clean modes, target triples, symlink handling (refuse symlinked roots, remove interior links without following), dry-run safety, and preservation of release builds in `debug-only` mode.
- `tests/cli_test.rs`: Validates dual-binary version/output parity and invalid-mode rejection.
- `tests/node/release-tools.test.cjs`: Validates target mapping, checksums, release identity, token scrubbing, and destructive Git confirmation.
- `tests/node/sync-version.test.cjs`: Validates version parsing, Cargo/npm manifest updates, all eight canonical tag-and-basename release URLs, and branch preflight against a safe local Git remote fixture before destructive cleanup.
- `tests/node/cargo-safe-update.test.cjs`: Validates 72h publish-age gating, emergency overrides, temp-lockfile transactions, env pins, and guarded `u`/`update:safe` entry points.
- `tests/node/check-cargo-update-policy.test.cjs`: Validates the unguarded-mutation scanner across scripts, workflows, and cargo aliases.
