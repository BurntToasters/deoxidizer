# deoxidizer

Free disk space from Rust and Tauri build artifacts without touching source
files. `deoxidizer` and `deox` are identical command names.

## Install

### macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.sh | bash
```

Install from a local release build:

```bash
./install.sh --from-source
```

### Windows

```powershell
irm https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.ps1 | iex
```

The one-line commands above follow the mutable `main` branch. For a release
review, inspect the bootstrap source at the candidate tag before running it:

- Bash: `https://github.com/BurntToasters/deoxidizer/blob/v<version>/install.sh`
- PowerShell: `https://github.com/BurntToasters/deoxidizer/blob/v<version>/install.ps1`

The tag pins the installer code; each installer still selects the latest
published release when it runs.

Release installers verify platform-specific SHA256 manifests before
installation:

- **macOS/Linux** (`install.sh`): authenticates the manifest with the pinned
  Minisign key (`release-signing-key.pub`, key ID `083B254A48C428E3`). The
  bootstrap requires the external `minisign` command for this first install.
- **Windows** (`install.ps1`): requires external `minisign.exe` to authenticate
  the manifest and valid Authenticode signatures from `BurntToasters` on both
  binaries.
- **`deox --update`**: verifies the manifest signature itself against the
  pinned key in-process; it does not need the external Minisign CLI.

Linux releases target x86_64 and aarch64 using static musl binaries. This avoids
glibc version coupling, but the project has not verified every Linux
distribution; the supported scope is those two target architectures.

## Quick start

```bash
deox setup --default         # create configuration
deox scan                    # inspect reclaimable artifacts
deox inspect <project-path>  # show one project's artifact breakdown
deox clean --dry-run         # preview cleanup (exact bytes)
deox clean --select          # pick projects interactively, then confirm
deox clean                   # confirm and clean
```

`clean` always shows project count and estimated space before prompting.
Use `--yes` only for deliberate non-interactive cleanup.

An empty `scan` and an empty `clean` both report
`No projects with build artifacts found.` (shared message).
`inspect` bypasses the configured scope, min-size, and ignored-projects
filters by design: an explicitly named project is inspected regardless of scan
preferences (`--scope` can still narrow recognition). The path may be the
project root or any file or folder inside it.

## Commands

| Command | Purpose |
| --- | --- |
| `deox setup [--default]` | Create or reset configuration |
| `deox scan` | Find Rust/Tauri build artifacts |
| `deox clean` | Remove build artifacts (all matching projects, or `--select`) |
| `deox inspect <project-path>` | Show one project's artifact breakdown |
| `deox settings show` | Display current configuration |
| `deox settings config ...` | Change configuration |
| `deox --update` | Verify and install the latest release |
| `deox --check-update` | Report whether a newer release exists |

Useful options:

```text
--path <dir>               Per-run projects directory override for this run only (scan, clean).
                           Stored default is set via `settings config --projects-dir`. `~/` expansion works.
--min-size-mb <mb>         Only show projects at least this large for this run (scan, clean).
                           Size is in MiB (1 MiB = 1048576 bytes).
                           Alias: --min-size. Overrides config. Boundary is >= (equal sizes included).
--older-than <days>        Only keep projects not modified in N days (scan: display filter only;
                           clean: pre-estimate filter). Projects with unknown modification time are dropped.
-m, --mode <mode>          Canonical modes: full, debug-only, incremental-only, deps-only (clean).
                           CLI is kebab-case, case-insensitive, plus aliases debug, incremental, deps
                           (e.g. `--mode DEBUG-ONLY` works; `--mode debug_only` is rejected, exit 2).
--scope <scope>            Override the scope filter for inspection (inspect, default: tauri-and-rust)
--dry-run                  Preview cleanup without changing files; reports the exact bytes the clean would free (clean)
-s, --select               Interactively choose which projects to clean; needs a terminal (clean)
--keep-bundles             In full mode, keep Tauri installer bundles under target/*/release/bundle (clean)
--json                     Machine-readable output on stdout (scan, clean)
-y, --yes                  Skip confirmation prompts (clean, settings reset); setup --yes skips
                           the overwrite confirmation when `--default` replaces an existing config file
-d, --default              Apply defaults without prompting (setup)
-u, --update               Verify and install the latest release. Cannot be combined with a subcommand.
--check-update             Report whether a newer release exists without installing it.
--config <path>            Global: use a specific config file instead of ~/.deox_config
                           (portable installs, testing, multiple profiles; no-op for inspect).
-h, --help                 Print help
-V, --version              Print version
```

Running bare `deox` with no subcommand scans with the stored configuration
(or defaults when no configuration exists).

Configure stored defaults without the wizard:

```text
deox settings config [--projects-dir <dir>] [--scope <scope>]
                     [--clean-behavior <behavior>] [--default-mode <mode>]
                     [--min-size-mb <mb>] [--ignored-projects <names>]
```

CLI flags (`--mode`, `settings config --scope/--clean-behavior/--default-mode`,
`inspect --scope`) accept canonical kebab-case values case-insensitively plus
listed aliases only: modes `debug`, `incremental`, `deps`; scopes `tauri`,
`both`, `all`, `rust`; behaviors `rm`, `remove`, `recycle`, `bin`
(e.g. `--mode DEBUG-ONLY` and `--scope TAURI` work; `--mode debug_only` and
`--scope tauri_only` are rejected, exit 2). The configuration file accepts
and stores only canonical kebab-case values.

`--projects-dir` values are stored as absolute paths (a leading `~` is kept
as-is), so they never depend on the directory you run `deox` from.

## Configuration

Configuration lives at `~/.deox_config`. Override per run with
`deox --config <path> <command>` (the home directory is resolved via OS
known folders, so `%USERPROFILE%` overrides are not honored on Windows).

Example (non-default values shown):

```json
{
  "version": 2,
  "projects_dir": "~/Documents/GitHub",
  "scope": "tauri-and-rust",
  "clean_behavior": "trash",
  "default_mode": "full",
  "min_size_mb": 100,
  "ignored_projects": ["important-project"]
}
```

Supported values:

- `scope`: `tauri-only`, `tauri-and-rust`, or `rust-only`.
- `clean_behavior`: `delete` or `trash`.
- `default_mode`: `full`, `debug-only`, `incremental-only`, or `deps-only`.
- `min_size_mb`: minimum artifact size in MiB; `0` includes everything.
- `ignored_projects`: project names excluded from scans and cleanup. A name
  matches a project or any package building into it, so ignoring one
  workspace member protects the workspace's shared `target/`.

Defaults are `tauri-only` / `delete` / `full` / `0` / `[]`.
`settings config` accepts the CLI aliases above; the file stores canonical
kebab-case values.

Saving settings (`deox settings config ...`) through a symlinked config file
writes the link's target and keeps the symlink.

Version-1 configuration files migrate automatically to version 2. Malformed or
unsupported-version configuration fails closed. Read-only scans may use defaults
when no configuration exists; `clean` requires setup.

## Cleanup modes

- `full`: remove entire `target/`.
- `debug-only`: remove `target/debug/` and `target/<triple>/debug/` while keeping release artifacts.
- `incremental-only`: remove `*/incremental/` compiler caches (depth ≤ 3, fail-closed).
- `deps-only`: remove `*/deps/` dependency artifacts (depth ≤ 3, fail-closed).

A workspace whose members share one `target/` appears once, named after the
workspace (its root package name, or its folder name for a virtual
workspace). The nearest workspace owns a package, as in Cargo.

Only Cargo build directories are cleaned. A `target/` folder qualifies only if
it contains Cargo's `CACHEDIR.TAG` or a regular `.rustc_info.json`. Any other
`target/` folder is reported with a `not a Cargo build directory` warning and is
never deleted. The same check runs again at clean time. A plain folder named
`target` that is not a build output is still scanned, so crates inside it are
found.

Cleanup supports cross-compilation triples: any non-`debug`/non-`release`
subdirectory of `target/` is treated as a triple candidate, and `debug-only`
cleans its real `debug/` directory. `incremental-only` and `deps-only` collect
matching directories up to depth 3 and intentionally under-clean deeper
layouts. Clean-time validation re-checks that every path is inside the
project's `target/`.

Deoxidizer never follows symlinks. A symlinked project root, `target/`, or
intermediate directory is refused. Symlinks *inside* `target/` (for example
the `.DirIcon` link in a Tauri AppImage bundle, or versioned `.so` links from
CMake-built crates) are removed as links; the files they point to are never
touched.

A project root may live on a different filesystem from its parent. On Unix,
clean planning refuses a mounted `target/` and mounted directories below it.
Linux uses `openat2` with `RESOLVE_NO_XDEV`, which also refuses same-device
bind mounts; if the kernel or environment cannot provide that operation, the
plan fails closed with an unsupported error. Other Unix systems compare device
IDs, so same-device bind mount detection is Linux-specific. Windows retains
handle-anchored path validation; this Unix mount-boundary check is not applied
there.

Full mode removes Tauri installer bundles (`target/release/bundle/`) along
with everything else and warns before it does; use `--keep-bundles` to keep
them. Projects with an active Cargo build (a held `.cargo-lock`) are skipped
and reported as `build in progress`.

`trash` mode is currently implemented on Linux with a private, same-filesystem
staging directory and verified file identity before moving the artifact to the
freedesktop Trash. If safe same-filesystem staging is unavailable, it returns
an error before moving or deleting files. On macOS and Windows, `trash` returns
a no-mutation error until a native backend can preserve the verified file
identity. Trash contents continue to occupy disk until emptied. A Trash failure
never falls back to permanent deletion.

Reported sizes are logical file bytes. Filesystem allocation, compression,
sparse-file holes, and clone sharing can make reclaimable disk space differ.
`clean --dry-run` and the confirmation prompt measure exactly the paths the
clean would remove, so the estimate matches what is freed unless files change
in between. Each breakdown row in `inspect` equals what the matching mode
frees; the `incremental/` and `deps/` rows overlap the `debug/`/`release/`
rows, so do not sum them.

## Scripting and stability

- `scan --json` and `clean --json` print a top-level `"schema_version": 1`.
  Paths in JSON output are strings decoded as lossy UTF-8.
- Exit codes:
  - `0`: success, or the user cancelled a confirmation.
  - `1`: runtime error, a partial clean (some projects failed), or analysis
    failures reported by `clean`.
  - `2`: usage error (unknown flag, invalid mode, scope, or value).
- From version 1.0, the JSON schema version, these exit codes, and the
  documented flags follow semantic versioning. Before 1.0 they can change
  between releases; check the CHANGELOG.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
| `Warning: skipping <path>: not a Cargo build directory (no CACHEDIR.TAG or .rustc_info.json)` | A folder named `target` lacks Cargo's build markers, so it is not a build output | Nothing is deleted. If it is a real Cargo target directory, rebuilding the project with Cargo recreates the markers |
| Names or paths show `U+FFFD` (�) characters | Control characters in a project, package, or path name were replaced so cloned repositories cannot inject terminal escape sequences | Expected behavior; `--json` output carries the raw names |
| `No projects with build artifacts found.` (scan, bare `deox`) | No in-scope projects passed filters | Run `settings show` to check scope, min-size, and ignored list; `inspect <path>` bypasses those filters |
| `No projects with build artifacts found.` (clean) | No in-scope projects passed filters (same shared message as scan) | Same as above; use `clean --dry-run` to preview |
| `Configuration error: invalid configuration JSON: ...` / `unsupported configuration version ...` | Malformed or future-version config | Delete `~/.deox_config` or run `deox setup --default` to recreate |
| `Scan failed: ...` | Bad path, unreadable root, or symlinked scan root | Check `--path` exists, is readable, and is a real directory (not a symlink) |
| `Warning: skipped N unreadable folder(s)` | Folders under the projects dir you cannot read (e.g. Docker volumes) | Harmless; they are left out of the scan |
| `skipped: build in progress` | Cargo is building that project right now | Re-run after the build finishes |
| `delete failed: ...` / `trash failed: ...` | Permissions, or full/unavailable Trash | Check permissions; for Trash, empty it or switch to `delete` behavior for deliberate permanent removal |
| `Clean completed with errors.` (Partial) | Some projects cleaned, some failed | Rerun `clean --dry-run`, then clean the single failing project to isolate |

## Build from source

```bash
git clone https://github.com/BurntToasters/deoxidizer.git
cd deoxidizer
cargo build --release --locked
./target/release/deoxidizer --version
./target/release/deox --version
```

## Maintainer release tooling

Release scripts use Node.js, npm, Cargo, the Tauri CLI signer, and GitHub CLI.
macOS Developer ID/notarization and Windows Azure Authenticode signing are
separate native signing requirements. Installed binaries remain local-first.
The release lookup endpoint is
`GET https://api.github.com/repos/BurntToasters/deoxidizer/releases/latest`.
`--check-update` fetches this metadata and compares versions; it does not
download an archive, checksum manifest, or signature. `--update` first tries
to recover a pending local update and can return without network access. If it
needs to check a release, it requests the same endpoint and, for a newer
version, downloads the platform archive, checksum manifest, and the manifest's
`.minisig` sidecar from GitHub release assets. Redirects are checked at every hop and may end at
`release-assets.githubusercontent.com` or `objects.githubusercontent.com`.
It verifies the signature and SHA256 before installing. There are no telemetry
or other runtime network calls.

Provision the release Minisign key outside the checkout. The public key is
checked in as [`release-signing-key.pub`](release-signing-key.pub), with ID
`083B254A48C428E3`. `npm run release:sign` invokes `cargo tauri signer sign`
and emits raw `.minisig` sidecars for the release payloads and checksum
manifest. Set `TAURI_SIGNING_PRIVATE_KEY_PATH` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` only on the provisioned signing machine;
the private key must match that public key ID and stay outside the repository.
Do not substitute a newly generated key without updating the pinned public key
across the runtime and installers. GPG is not used. The shell and
PowerShell first-install bootstraps need an installed `minisign` executable;
the updater and release verifier use the pinned in-process verifier.

```bash
npm ci

npm run u -- 0.1.1                 # e.g. next version: safe dep updates (72h/3d age-gated) + sync version across manifests and changelog
npm run update:safe                # dep lockfile updates only, no version sync
DEOX_RELEASE_CONFIRM=YES npm run r  # hard-reset to origin/main, reinstall, prune gone branches
DEOX_RELEASE_CONFIRM=YES npm run b  # hard-reset to origin/beta, reinstall

# Run the matching command on each native OS/architecture release VM. Use a
# clean detached checkout of the exact candidate tag/commit on all six VMs.
npm run release:windows:x64    # Windows x86_64: first Windows uploader creates the draft
npm run release:windows:arm64  # Windows aarch64
npm run release:linux:x64     # Linux x86_64 with musl-tools; waits for the draft
npm run release:linux:arm64   # Linux aarch64 with musl-tools; waits for the draft
npm run release:macos:x64     # macOS x86_64 with Developer ID + notarization; waits
npm run release:macos:arm64   # macOS aarch64 with Developer ID + notarization; waits

# Optional staged-artifact upload path:
npm run release:upload

npm run release:verify:draft
# Publish only after review and final approval; this is an irreversible transition.
DEOX_RELEASE_CONFIRM=YES npm run release:publish -- --yes
```

Each command builds one native target and uploads its signed evidence to the
same GitHub draft. `release:verify:draft` requires exactly 28 assets: six
archives, six platform checksum manifests, two Windows setup installers, and
their 14 `.minisig` sidecars. It also verifies the tag commit and release
identity. The Windows x86_64 command creates the draft; Windows arm64 reuses
it, and Linux/macOS wait for it. The `release:publish` gate re-verifies the
remote draft and its signatures before changing its visibility.

macOS targets must run on native macOS release VMs, not GitHub-hosted macOS
runners. Each VM needs a Developer ID certificate and pre-provisioned
`APPLE_KEYCHAIN_PROFILE`; notarization must return `Accepted`. Windows targets
need Azure Artifact Signing credentials and tools. Linux targets are built
only on a Linux host matching the target architecture and require `musl-gcc`
(provided by `musl-tools`).

CI (`.github/workflows/ci.yml`) must be a required status check on `main` in
branch protection. `main` stayed red for weeks while the check was not enforced.
Dependabot ignores semver-major Cargo updates; major bumps go through
`npm run update:safe` together with the code changes that they need.

Release publication requires `gh auth login` on each release VM. Do not publish
until the candidate tag and all six native build proofs are inspectable and
`npm run release:verify:draft` succeeds with all 28 assets.

### Stable updater rehearsal

Use a disposable machine with the previous stable release installed. Drafts are
excluded from `/releases/latest`, so the first stable release cannot be
rehearsed through the public updater before publication. After the approved
publish, run `deox --check-update` and confirm it reports the candidate; then
run `deox --update` and verify both `deox --version` and
`deoxidizer --version`. `--check-update` alone does not exercise artifact,
signature, checksum, or installation handling. Record the host OS/architecture,
commands, version output, and any error as the first stable updater evidence.

Release notes live in [`CHANGELOG.md`](CHANGELOG.md) and follow the
[BCLS](https://github.com/BurntToasters/BCLS) standard.

## Support

Enjoying deoxidizer? Consider [Supporting Me!](https://rosie.run/support).

Contributions are welcome via GitHub issues and pull requests.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
