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
identity. Permanent `delete` cleanup remains available on every supported OS.
Trash contents continue to occupy disk until emptied. A Trash failure never
falls back to permanent deletion.

Future backend candidates for macOS are `FileManager.trashItem` with a
`fileReferenceURL`; for Windows, `IFileOperation` with
`FOFX_RECYCLEONDELETE` and its per-item placement callback. These are research
leads only. Any implementation must prove identity across races, restore the
original item after a failed move, and fail closed without mutation whenever
identity, callback, or restore proof is unavailable.

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

### Stable v0.1.0 release runbook

1. Freeze a clean candidate checkout at the exact commit intended for `v0.1.0`.
   Run the local gates and require branch-protected CI to pass on this same
   commit:

   ```bash
   npm ci
   npm run quality
   cargo deny check advisories bans licenses sources
   ```

2. Preflight the provisioned signing key before release builds. Keep the
   scratch payload outside the checkout. `signFile` invokes `cargo tauri
   signer sign`, converts Tauri's wrapper to a raw `.minisig`, and verifies it
   with the pinned in-process verifier used by `deox --update`:

   ```bash
   scratch="$(mktemp)"
   trap 'rm -f "$scratch" "$scratch.minisig"' EXIT
   printf 'deoxidizer v0.1.0 signing preflight\n' > "$scratch"
   node -e 'const s=require("./scripts/minisign-sign.cjs");
   s.signFile(process.argv[1],s.minisignEnvironment(process.env));' "$scratch"
   ```

   The provisioned key must match key ID `083B254A48C428E3`; keep its private
   file outside the checkout and never put passwords in command arguments.

3. Before any upload, check the actual GitHub `v0.1.0` tag and release for a
   collision. Stop if either already exists or points elsewhere. Create and
   push an annotated `v0.1.0` tag at the candidate commit, then resolve the
   remote tag and require its peeled commit SHA to equal `git rev-parse HEAD`.
   GitHub `target_commitish` alone does not bind a tag. All six clean native
   build checkouts must use that exact commit.

4. Stage all six signed builds locally, with no `--upload`, on matching native
   OS/architecture VMs. Wait until the release-tool fixture checks in
   `npm run quality` and local verification of all six stages pass before
   uploading any asset:

   ```bash
   node scripts/release.cjs windows x86_64
   node scripts/release.cjs windows aarch64
   node scripts/release.cjs linux x86_64
   node scripts/release.cjs linux aarch64
   node scripts/release.cjs darwin x86_64
   node scripts/release.cjs darwin aarch64
   ```

   Each command stages one target under `release/staging/<rust-target>` and
   runs its local verifier. To repeat local verification for an isolated stage,
   set `DEOX_RELEASE_STAGE_DIR` to `release/staging/<rust-target>` and run
   `npm run release:verify`. macOS requires Developer ID and accepted
   notarization; Windows requires verified Azure Authenticode; Linux requires
   a matching native host and `musl-gcc`.

   Before uploading, run the offline signed candidate rehearsal on native
   Windows x86_64 for a complete pass; it verifies the staged archive, pinned
   signature, native proof, both invokers, recovery, tamper rejection, and
   interrupted replacement. Retain its transcript. On every native
   OS/architecture VM, install/extract the exact staged candidate (not the
   public latest-release bootstrap) into a disposable prefix, verify
   `deoxidizer --version` and `deox --version`, and capture `clean --dry-run`
   plus a confirmed cleanup against a disposable marked Cargo fixture and
   isolated config. Linux must prove Trash placement; macOS and Windows must
   prove Trash refuses before mutation; permanent deletion must work on all
   three OSes. On Windows, use a clean account with a short user PATH to install
   the staged NSIS setup, verify the user PATH and both invokers, uninstall it,
   and verify its app files and PATH entry are removed. Keep transcripts,
   before/after fixture hashes, and Windows PATH and uninstall evidence under
   `target/e2e-artifacts/`. Do not upload until all these checks pass.

   ```bash
   node scripts/rehearse-signed-update.cjs --stage-dir release/staging/x86_64-pc-windows-msvc --binary-dir target/x86_64-pc-windows-msvc/release --output target/e2e-artifacts/signed-update-rehearsal-windows-x64.json
   ```

5. Upload serially from each target's native VM with `gh auth login`. The
   Windows x86_64 upload creates the draft; do not overlap uploaders:

   ```bash
   npm run release:upload -- --target x86_64-pc-windows-msvc
   npm run release:upload -- --target aarch64-pc-windows-msvc --wait
   npm run release:upload -- --target x86_64-unknown-linux-musl --wait
   npm run release:upload -- --target aarch64-unknown-linux-musl --wait
   npm run release:upload -- --target x86_64-apple-darwin --wait
   npm run release:upload -- --target aarch64-apple-darwin --wait
   ```

6. After all uploads, require
   `npm run release:verify:draft -- --integrity` to pass. The draft must contain
   exactly 28 assets: six archives, six platform checksum manifests, two
   Windows setup installers, and 14 `.minisig` sidecars. Verify the remote tag
   resolves to the candidate SHA and inspect all six native proofs. The release
   owner must explicitly approve before the irreversible publish step:

   ```bash
   DEOX_RELEASE_CONFIRM=YES npm run release:publish -- --yes
   ```

7. After publishing v0.1.0, perform a fresh public discovery check only with
   `deox --check-update`; record the host and output. At the next stable release,
   after that candidate is public, use a disposable installation of v0.1.0 to
   run the first real GitHub upgrade:

   ```bash
   deox --check-update
   deox --update
   deox --version
   deoxidizer --version
   ```

   Record the host OS/architecture, command output, and before/after versions
   under `target/e2e-artifacts/`. This public upgrade is separate from the
   offline signed candidate rehearsal above.

CI (`.github/workflows/ci.yml`) must be a required status check on `main` in
branch protection and must pass on the exact stable candidate commit.
Dependabot ignores semver-major Cargo updates; major bumps go through
`npm run update:safe` together with the code changes that they need.

Release notes live in [`CHANGELOG.md`](CHANGELOG.md) and follow the
[BCLS](https://github.com/BurntToasters/BCLS) standard.

## Support

Enjoying deoxidizer? Consider [Supporting Me!](https://rosie.run/support).

Contributions are welcome via GitHub issues and pull requests.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
