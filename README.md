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

Release installers verify platform-specific SHA256 manifests before
installation:

- **macOS/Linux** (`install.sh`): authenticates the manifest with the pinned
  release key (`CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F`) using `gpg`. Only
  this first install needs `gpg` (`brew install gnupg`, `apt install gnupg`).
- **Windows** (`install.ps1`): requires valid Authenticode signatures from
  `BurntToasters` on both binaries; also checks the manifest signature when
  `gpg.exe` is available.
- **`deox --update`**: verifies the manifest signature itself against the
  pinned key. No `gpg` needed.

Linux binaries are static (musl), so they run on any distribution.

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

Full mode removes Tauri installer bundles (`target/release/bundle/`) along
with everything else and warns before it does; use `--keep-bundles` to keep
them. Projects with an active Cargo build (a held `.cargo-lock`) are skipped
and reported as `build in progress`.

`trash` behavior moves directories via the OS Trash / Recycle Bin (Finder
Trash on macOS, Recycle Bin on Windows, freedesktop Trash on Linux). Trashed
files still occupy disk until the Trash is emptied. A `trash failed` error does
not fall back to permanent deletion; rerun with `delete` behavior only for
deliberate permanent removal, and empty the Trash to reclaim trashed space.
Delete failures are usually permissions; trash failures are usually permissions
or a full/unavailable Trash. On macOS, trashing uses `NSFileManager`, so it
needs no Finder permission prompt (Finder's "Put Back" may not be offered). On
Windows, items larger than the Recycle Bin's size limit may be deleted
permanently by Windows; use `--dry-run` first for very large targets.

Reported sizes are logical file bytes. Filesystem allocation, compression,
sparse-file holes, and clone sharing can make reclaimable disk space differ.
`clean --dry-run` and the confirmation prompt measure exactly the paths the
clean would remove, so the estimate matches what is freed unless files change
in between. Each breakdown row in `inspect` equals what the matching mode
frees; the `incremental/` and `deps/` rows overlap the `debug/`/`release/`
rows, so do not sum them.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
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

Release scripts use Node.js, npm, Cargo, GPG, and optionally GitHub CLI.
Installed binaries remain local-first; only `deox --update` performs runtime
network access.

```bash
npm ci

npm run u -- 0.1.1                 # e.g. next version: safe dep updates (72h/3d age-gated) + sync version across manifests and changelog
npm run update:safe                # dep lockfile updates only, no version sync
DEOX_RELEASE_CONFIRM=YES npm run r  # hard-reset to origin/main, reinstall, prune gone branches
DEOX_RELEASE_CONFIRM=YES npm run b  # hard-reset to origin/beta, reinstall

# On each release VM, prefix with DEOX_RELEASE_CONFIRM=YES npm run r first
# (Windows creates the single draft; Linux/macOS wait for that draft):
npm run release:windows  # Windows release VM: create draft + upload Windows artifacts
npm run release:linux    # Linux release VM (x86_64 or arm64 host, musl-tools installed): wait for draft + upload
npm run release:macos    # macOS release VM: wait for draft + upload

# Optional staged-artifact upload path:
npm run release:upload

npm run release:verify:draft
DEOX_RELEASE_CONFIRM=YES npm run release:publish -- --yes  # irreversible; see scripts/publish-release.cjs
```

Release publication requires `gh auth login` on each release VM. The Windows
VM creates the single draft; Linux and macOS commands wait for that draft before
uploading their signed archives and platform-specific checksum manifests. Draft
verification must pass for all supported targets before publishing.

Release notes live in [`CHANGELOG.md`](CHANGELOG.md) and follow the
[BCLS](https://github.com/BurntToasters/BCLS) standard.

## Support

Enjoying deoxidizer? Consider [Supporting Me!](https://rosie.run/support).

Contributions are welcome via GitHub issues and pull requests.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
