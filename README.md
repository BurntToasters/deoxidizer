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
deox setup --default   # create configuration
deox scan              # inspect reclaimable artifacts
deox clean --dry-run   # preview cleanup
deox clean             # confirm and clean
```

`clean` always shows project count and estimated space before prompting.
Use `--yes` only for deliberate non-interactive cleanup.

## Commands

| Command | Purpose |
| --- | --- |
| `deox setup [--default]` | Create or reset configuration |
| `deox scan` | Find Rust/Tauri build artifacts |
| `deox clean` | Remove selected artifacts |
| `deox inspect <path>` | Show one project's artifact breakdown |
| `deox settings show` | Display current configuration |
| `deox settings config ...` | Change configuration |
| `deox --update` | Verify and install the latest release |

Useful options:

```text
--path <dir>             Scan a different projects directory
--min-size <mb>          Ignore smaller artifacts during scan
--mode <mode>            full, debug-only, incremental-only, or deps-only
--older-than <days>      Clean artifacts older than this age
--dry-run                Preview cleanup without changing files
--yes                    Skip cleanup confirmation
```

## Configuration

Configuration lives at `~/.deox_config`:

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

Version-1 configuration files migrate automatically to version 2. Malformed or
unsupported-version configuration fails closed. Read-only scans may use defaults
when no configuration exists; `clean` requires setup.

## Cleanup modes

- `full`: remove entire `target/`.
- `debug-only`: remove debug artifacts while keeping release artifacts.
- `incremental-only`: remove incremental compiler caches.
- `deps-only`: remove dependency build artifacts.

Cleanup supports workspace target directories and cross-compilation triples.
Symlinks are rejected. Deoxidizer never follows links or removes paths outside
a validated project `target/` directory.

Reported sizes are logical file bytes. Filesystem allocation, compression,
sparse-file holes, and clone sharing can make reclaimable disk space differ.

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

npm run u -- 0.1.1                 # sync version across manifests and changelog
DEOX_RELEASE_CONFIRM=YES npm run r  # sync main, test, prune
DEOX_RELEASE_CONFIRM=YES npm run b  # sync beta, test

# On the Windows release VM: create the draft and upload Windows artifacts.
DEOX_RELEASE_CONFIRM=YES npm run r
npm run release:windows

# On Linux/macOS release VMs: wait for the Windows-created draft and upload.
DEOX_RELEASE_CONFIRM=YES npm run r
npm run release:linux
DEOX_RELEASE_CONFIRM=YES npm run r
npm run release:macos

# Optional staged-artifact upload path:
npm run release:upload

npm run release:verify:draft
npm run release:publish -- --yes
```

Release publication requires `gh auth login` on each release VM. The Windows
VM creates the single draft; Linux and macOS commands wait for that draft before
uploading their signed archives and platform-specific checksum manifests. Draft
verification must pass for all supported targets before publishing.

Release notes live in [`CHANGELOG.md`](CHANGELOG.md) and follow the
[BCLS](https://github.com/BurntToasters/BCLS) standard.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
