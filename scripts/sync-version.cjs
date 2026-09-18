'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const packagePath = path.join(root, 'package.json');
const packageLockPath = path.join(root, 'package-lock.json');
const cargoPath = path.join(root, 'Cargo.toml');
const cargoLockPath = path.join(root, 'Cargo.lock');
const changelogPath = path.join(root, 'CHANGELOG.md');

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function validateVersion(version) {
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:alpha|beta|rc)\.(?:0|[1-9]\d*))?$/.test(version)) {
    throw new Error(`Invalid release version: ${version}`);
  }
  return version;
}

// Single shared prerelease predicate. Strict suffix mirrors validateVersion
// above (lowercase alpha/beta/rc + dot + numeric); all release scripts must
// reuse this instead of duplicating ad-hoc prerelease regexes.
function isPrerelease(version) {
  return /-(?:alpha|beta|rc)\.(?:0|[1-9]\d*)$/.test(String(version));
}

function cargoVersion(cargo) {
  const match = cargo.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('Cargo.toml has no package version');
  return match[1];
}

function updateCargoToml(cargo, version) {
  const packageStart = cargo.indexOf('[package]');
  if (packageStart < 0) throw new Error('Cargo.toml has no [package] section');
  const nextSection = cargo.indexOf('\n[', packageStart + '[package]'.length);
  const packageSection = cargo.slice(
    packageStart,
    nextSection < 0 ? cargo.length : nextSection,
  );
  const matches = packageSection.match(/^version\s*=\s*"[^"]*"/gm) || [];
  if (matches.length !== 1) {
    throw new Error(`Cargo.toml package version must appear exactly once; found ${matches.length}`);
  }
  const updatedSection = packageSection.replace(
    /^version\s*=\s*"[^"]*"/m,
    `version = "${version}"`,
  );
  return cargo.replace(packageSection, updatedSection);
}

function updateCargoLock(lockfile, version) {
  const pattern = /(\[\[package\]\]\r?\nname = "deoxidizer"\r?\nversion = )"[^"]*"/g;
  const matches = lockfile.match(pattern) || [];
  if (matches.length !== 1) {
    throw new Error(`Cargo.lock deoxidizer package must appear exactly once; found ${matches.length}`);
  }
  return lockfile.replace(pattern, `$1"${version}"`);
}

function updatePackageLock(lockfile, version) {
  const lock = JSON.parse(lockfile);
  if (!lock.packages?.['']) {
    throw new Error('package-lock.json is missing packages[""]');
  }
  lock.version = version;
  lock.packages[''].version = version;
  return `${JSON.stringify(lock, null, 2)}\n`;
}

function updateChangelog(changelog, fromVersion, toVersion) {
  if (fromVersion === toVersion) return changelog;
  const oldHeading = `## Changes in \`v${fromVersion}:\``;
  const newHeading = `## Changes in \`v${toVersion}:\``;
  if (!changelog.includes(oldHeading)) {
    throw new Error(
      `CHANGELOG.md is missing current release heading ${oldHeading} ` +
        `(expected BCLS heading '## Changes in \`v<version>:\`'). ` +
        `CHANGELOG head: ${JSON.stringify(changelog.slice(0, 200))}`,
    );
  }

  const tableStart = changelog.indexOf('# ⬇️ Downloads');
  const tableEnd = changelog.indexOf('\n> [!IMPORTANT]', tableStart);
  if (tableStart < 0 || tableEnd <= tableStart) {
    throw new Error(
      "CHANGELOG.md download table markers not found (expected '# ⬇️ Downloads' " +
        `followed by '\\n> [!IMPORTANT]'). CHANGELOG head: ${JSON.stringify(changelog.slice(0, 200))}`,
    );
  }
  const table = changelog.slice(tableStart, tableEnd).replace(
    /\/releases\/download\/v[^/]+\//g,
    `/releases/download/v${toVersion}/`,
  );
  return `${changelog.slice(0, tableStart)}${table}${changelog
    .slice(tableEnd)
    .replace(oldHeading, newHeading)}`;
}

function syncVersion(versionArgument, rootOverride) {
  const base = rootOverride || root;
  const packageJson = readJson(path.join(base, 'package.json'));
  const packageVersion = validateVersion(String(packageJson.version));
  const cargo = fs.readFileSync(path.join(base, 'Cargo.toml'), 'utf8');
  const currentCargoVersion = validateVersion(cargoVersion(cargo));

  // No explicit target: package.json is the source of truth (edit it, then
  // run bare `npm run u`). The changelog moves from the version the rest of
  // the repo is at; a half-bumped Cargo.toml still fails closed via the
  // missing-heading error below before anything is written.
  const version = validateVersion(versionArgument || packageVersion);
  const fromVersion = versionArgument ? packageVersion : currentCargoVersion;
  const packageLock = fs.readFileSync(path.join(base, 'package-lock.json'), 'utf8');
  const cargoLock = fs.readFileSync(path.join(base, 'Cargo.lock'), 'utf8');
  const changelog = fs.readFileSync(path.join(base, 'CHANGELOG.md'), 'utf8');

  const updatedPackage = { ...packageJson, version };
  const updatedPackageLock = updatePackageLock(packageLock, version);
  const updatedCargo = updateCargoToml(cargo, version);
  const updatedCargoLock = updateCargoLock(cargoLock, version);
  const updatedChangelog = updateChangelog(changelog, fromVersion, version);

  const changes = [
    [path.join(base, 'package.json'), `${JSON.stringify(updatedPackage, null, 2)}\n`, JSON.stringify(packageJson, null, 2) + '\n'],
    [path.join(base, 'package-lock.json'), updatedPackageLock, packageLock],
    [path.join(base, 'Cargo.toml'), updatedCargo, cargo],
    [path.join(base, 'Cargo.lock'), updatedCargoLock, cargoLock],
    [path.join(base, 'CHANGELOG.md'), updatedChangelog, changelog],
  ];
  for (const [filePath, updated, original] of changes) {
    if (updated !== original) {
      fs.writeFileSync(filePath, updated);
      console.log(`${path.relative(base, filePath)} → ${version}`);
    }
  }
  return version;
}

if (require.main === module) {
  try {
    const versionArgument = process.argv[2];
    syncVersion(versionArgument);
  } catch (error) {
    console.error(`✗ Version sync failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = {
  cargoVersion,
  isPrerelease,
  syncVersion,
  updateCargoLock,
  updateCargoToml,
  updateChangelog,
  updatePackageLock,
  validateVersion,
};
