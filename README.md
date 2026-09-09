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
installation and authenticate manifests with the pinned release key
(`CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F`). Windows binaries also require
valid Authenticode signatures from `BurntToasters`. Release update/install
flows require `gpg` for manifest signature verification.

## Quick start

```bash
deox setup --default         # create configuration
deox scan                    # inspect reclaimable artifacts
deox inspect <project-path>  # show one project's artifact breakdown
deox clean --dry-run         # preview cleanup
deox clean                   # confirm and clean
```

`clean` always shows project count and estimated space before prompting.
Use `--yes` only for deliberate non-interactive cleanup.

An empty `scan` reports `No projects with build artifacts found.`, while an
empty `clean` reports `No projects with cleanable artifacts found.`.
`inspect` bypasses the configured scope, min-size, and ignored-projects
filters by design: an explicitly named project is inspected regardless of scan
preferences (`--scope` can still narrow recognition).

## Commands

| Command | Purpose |
| --- | --- |
| `deox setup [--default]` | Create or reset configuration |
| `deox scan` | Find Rust/Tauri build artifacts |
| `deox clean` | Remove selected artifacts |
| `deox inspect <project-path>` | Show one project's artifact breakdown |
| `deox settings show` | Display current configuration |
| `deox settings config ...` | Change configuration |
| `deox --update` | Verify and install the latest release |

Useful options:

```text
--path <dir>               Per-run projects directory override for this run only (scan, clean).
                           Stored default is set via `settings config --projects-dir`. `~/` expansion works.
--min-size-mb <mb>         Only show projects at least this large for this run (scan, clean).
                           Alias: --min-size. Overrides config. Boundary is >= (equal sizes included).
--older-than <days>        Only keep projects not modified in N days (scan: display filter only;
                           clean: pre-estimate filter). Projects with unknown modification time are dropped.
-m, --mode <mode>          Canonical modes: full, debug-only, incremental-only, deps-only (clean).
                           CLI aliases: debug, incremental, deps. `settings config --default-mode`
                           accepts loose names (case-insensitive, `_`/`-`, aliases).
--scope <scope>            Override the scope filter for inspection (inspect, default: tauri-and-rust)
--dry-run                  Preview cleanup without changing files (clean)
-y, --yes                  Skip cleanup confirmation (clean, settings reset)
-d, --default              Apply defaults without prompting (setup)
-u, --update               Global: verify and install the latest release. Usable alongside subcommands.
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

`settings config` accepts loose names for `--scope`, `--clean-behavior`, and
`--default-mode` (case-insensitive, `_`/`-` interchangeable, aliases such as
`debug`, `incremental`, `deps`, `tauri`, `both`, `rust`). The configuration
file stores canonical kebab-case values.

## Configuration

Configuration lives at `~/.deox_config`.

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
- `min_size_mb`: minimum artifact size; `0` includes everything.
- `ignored_projects`: project names excluded from scans and cleanup.

Defaults are `tauri-only` / `delete` / `full` / `0` / `[]`.
`settings config` accepts loose aliases, but the file stores canonical
kebab-case values.

Version-1 configuration files migrate automatically to version 2. Malformed or
unsupported-version configuration fails closed. Read-only scans may use defaults
when no configuration exists; `clean` requires setup.

## Cleanup modes

- `full`: remove entire `target/`.
- `debug-only`: remove `target/debug/` and `target/<triple>/debug/` while keeping release artifacts.
- `incremental-only`: remove `*/incremental/` compiler caches (depth ≤ 3, fail-closed).
- `deps-only`: remove `*/deps/` dependency artifacts (depth ≤ 3, fail-closed).

Cleanup supports workspace target directories and cross-compilation triples.
Any non-`debug`/non-`release` subdirectory of `target/` is treated as a triple
candidate: when it contains a real `debug/` directory, `debug-only` cleans it.
The match is intentionally broad and is not narrowed to a known triple list.
`incremental-only` and `deps-only` collect matching directories up to depth 3
and intentionally under-clean deeper layouts rather than risk over-deletion;
clean-time path validation re-checks containment.
Symlinks are rejected. Deoxidizer never follows links or removes paths outside
a validated project `target/` directory.

`trash` behavior moves directories via the OS Trash / Recycle Bin (Finder
Trash on macOS, Recycle Bin on Windows, freedesktop Trash on Linux). Trashed
files still occupy disk until the Trash is emptied. A `trash failed` error does
not fall back to permanent deletion; rerun with `delete` behavior only for
deliberate permanent removal, and empty the Trash to reclaim trashed space.
Delete failures are usually permissions; trash failures are usually permissions
or a full/unavailable Trash.

Reported sizes are logical file bytes. Filesystem allocation, compression,
sparse-file holes, and clone sharing can make reclaimable disk space differ.
`clean --dry-run` walks the filesystem at clean time and is the canonical
reclaimable-byte source; pre-clean estimates from scan breakdowns may diverge
if artifacts changed between scan and clean. Breakdown subtotals overlap
(`incremental/` and `deps/` bytes are also counted inside `debug/`/`release/`),
so do not sum the columns.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
| `No projects with build artifacts found.` (scan, bare `deox`) | No in-scope projects passed filters | Run `settings show` to check scope, min-size, and ignored list; `inspect <path>` bypasses those filters |
| `No projects with cleanable artifacts found.` (clean) | Same filters, clean-time wording differs by design | Same as above; use `clean --dry-run` to preview |
| `Configuration error: invalid configuration JSON: ...` / `unsupported configuration version ...` | Malformed or future-version config | Delete `~/.deox_config` or run `deox setup --default` to recreate |
| `Scan failed: ...` | Bad path, unreadable root, or symlinked scan root | Check `--path` exists, is readable, and is a real directory (not a symlink) |
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

npm run u -- 0.1.1                 # e.g. next version: sync version across manifests and changelog
DEOX_RELEASE_CONFIRM=YES npm run r  # sync main, test, prune
DEOX_RELEASE_CONFIRM=YES npm run b  # sync beta, test

# On each release VM, prefix with DEOX_RELEASE_CONFIRM=YES npm run r first
# (Windows creates the single draft; Linux/macOS wait for that draft):
npm run release:windows  # Windows release VM: create draft + upload Windows artifacts
npm run release:linux    # Linux release VM: wait for draft + upload
npm run release:macos    # macOS release VM: wait for draft + upload

# Optional staged-artifact upload path:
npm run release:upload

npm run release:verify:draft
npm run release:publish -- --yes  # irreversible; see scripts/publish-release.cjs
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
