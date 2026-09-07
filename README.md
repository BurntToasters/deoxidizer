# deoxidizer

Fast, safe disk space cleaner for Rust and Tauri build artifacts.

`deoxidizer` and `deox` are equivalent command invokers. Both use the same
library, configuration, scanner, cleaner, and updater.

## Installation

### macOS/Linux
```bash
curl -fsSL https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.sh | bash
# or from a clone after a release build:
./install.sh --from-source
```

### Windows
```powershell
irm https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.ps1 | iex
```

Installers require an exact entry in platform-scoped checksum manifests before
installing a GitHub release archive.

## Usage

Both `deoxidizer` and `deox` can be used interchangeably as command names.

* `deox setup [--default]`: Initialize your configuration.
* `deox scan [--path <dir>] [--min-size <mb>]`: Scan build artifacts.
* `deox clean [-y] [--mode <mode>] [--dry-run] [--older-than <days>]`: Clean projects.
* `deox settings <show|config|reset>`: Manage your `deoxidizer` settings.
* `deox inspect <path>`: Deep-dive inspection of a specific project's target directory.
* `deox --update`: Update the `deoxidizer` binary to the latest version.
* `deox --version`: Show the current version.

`deox clean` prints project count and estimated reclaimable bytes, then prompts
for confirmation. Use `--yes` only for explicit non-interactive cleaning.
`--dry-run`, `scan`, and `inspect` do not mutate files.

## Configuration

Configuration is stored in `~/.deox_config` as JSON.

Example:
```json
{
  "version": 1,
  "projects_dir": "/Users/dev/Projects",
  "scope": "tauri-and-rust",
  "clean_behavior": "trash",
  "default_mode": "full",
  "min_size_mb": 100,
  "ignored_projects": [
    "important-project"
  ]
}
```

Fields:
- `version`: Config version.
- `projects_dir`: Base directory to scan for projects.
- `scope`: What projects to scan (`tauri-only`, `rust-only`, `tauri-and-rust`).
- `clean_behavior`: Action to take (`delete` permanently, or send to `trash`).
- `default_mode`: Default cleaning mode.
- `min_size_mb`: Minimum target folder size to consider.
- `ignored_projects`: List of folder names to skip.

## Clean Modes

| Mode | Description |
|---|---|
| `full` | Deletes the entire `target/` directory. |
| `debug-only` | Deletes `target/debug/` but preserves `release/`. |
| `incremental-only` | Deletes incremental compilation caches only. |
| `deps-only` | Deletes dependency compilation units only. |

Symlinked artifact paths are rejected. The cleaner never follows links or
removes paths outside a validated project `target/` directory.

## Building from source

Ensure you have Rust and Cargo installed, then run:

```bash
git clone https://github.com/BurntToasters/deoxidizer.git
cd deoxidizer
cargo build --release --locked
./target/release/deoxidizer --version
./target/release/deox --version
```

## Release tooling

Release automation uses Node.js and npm only when explicitly invoked. It
mirrors IYERIS release coordination while keeping installed binaries
network-free except for `deox --update`.

```bash
npm ci
npm run r                         # refresh main, verify, prune
npm run b                         # refresh beta, verify
npm run release:linux             # Linux x86_64
npm run release:linux:arm64      # Linux aarch64
npm run release:macos             # macOS host target
npm run release:windows           # Windows host target
npm run release:verify:draft      # verify complete multi-target draft
npm run release:publish -- --yes  # publish only after draft verification
```

`npm run r` and `npm run b` perform destructive Git synchronization. They
require `DEOX_RELEASE_CONFIRM=YES`. Release publication additionally requires
GitHub CLI authentication with `gh auth login`. Each target publishes a
platform-scoped checksum manifest; draft verification requires all supported
targets before `release:publish`.

## License

This project is licensed under the GNU General Public License v3.0 or later
(GPL-3.0-or-later). See `LICENSE`.
