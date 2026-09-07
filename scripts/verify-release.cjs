'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const {
  assertGitHubCliAuthenticated,
  githubApi,
  repository,
} = require('./github-cli.cjs');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error('Cargo.toml has no version');
const tag = `v${version}`;

function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function parseChecksumManifest(text) {
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = line.match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (!match) throw new Error(`Invalid checksum line: ${line}`);
    if (entries.has(match[2])) throw new Error(`Duplicate checksum entry: ${match[2]}`);
    entries.set(match[2], match[1].toLowerCase());
  }
  return entries;
}

function verifyChecksum(filePath, expected) {
  return sha256(filePath).toLowerCase() === expected.toLowerCase();
}

function archiveEntries(filePath) {
  const command = filePath.endsWith('.zip') ? 'unzip' : 'tar';
  const args = filePath.endsWith('.zip')
    ? ['-Z1', filePath]
    : ['-tzf', filePath];
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.error || result.status !== 0) {
    throw new Error(`Cannot inspect archive ${path.basename(filePath)}`);
  }
  return new Set(String(result.stdout).split(/\r?\n/).filter(Boolean));
}

function expectedArchives() {
  if (!fs.existsSync(releaseDir)) return [];
  return fs
    .readdirSync(releaseDir)
    .filter(
      (name) =>
        name.startsWith(`deoxidizer-v${version}-`) &&
        (name.endsWith('.tar.gz') || name.endsWith('.zip')),
    )
    .map((name) => path.join(releaseDir, name));
}

function allReleaseFiles() {
  if (!fs.existsSync(releaseDir)) return [];
  return fs
    .readdirSync(releaseDir)
    .filter((name) => !name.startsWith('.') && fs.statSync(path.join(releaseDir, name)).isFile())
    .map((name) => path.join(releaseDir, name));
}

function verifyLocal() {
  const archives = expectedArchives();
  if (archives.length === 0) throw new Error(`No release archives found in ${releaseDir}`);
  const checksumPaths = allReleaseFiles().filter((filePath) =>
    /^SHA256SUMS(?:-[a-z0-9_-]+)?\.txt$/i.test(path.basename(filePath)),
  );
  if (checksumPaths.length === 0) throw new Error('No SHA256SUMS*.txt manifest found');
  const entries = new Map();
  for (const checksumPath of checksumPaths) {
    for (const [name, digest] of parseChecksumManifest(fs.readFileSync(checksumPath, 'utf8'))) {
      if (entries.has(name) && entries.get(name) !== digest) {
        throw new Error(`Conflicting checksum entries for ${name}`);
      }
      entries.set(name, digest);
    }
  }

  const signedAssets = allReleaseFiles().filter(
    (filePath) =>
      !checksumPaths.some((checksumPath) => path.basename(checksumPath) === path.basename(filePath)) &&
      !path.basename(filePath).endsWith('.asc'),
  );
  for (const filePath of signedAssets) {
    const name = path.basename(filePath);
    const expected = entries.get(name);
    if (!expected || !verifyChecksum(filePath, expected)) {
      throw new Error(`Checksum mismatch or missing entry: ${name}`);
    }
    if (filePath.endsWith('.tar.gz') || filePath.endsWith('.zip')) {
      const names = archiveEntries(filePath);
      for (const required of ['deoxidizer', 'deox', 'LICENSE']) {
        const windowsName = `${required}.exe`;
        if (!names.has(required) && !names.has(windowsName)) {
          throw new Error(`${name} is missing ${required}`);
        }
      }
    }
    if (
      process.env.DEOX_ALLOW_UNSIGNED_RELEASE !== '1' &&
      !fs.existsSync(`${filePath}.asc`)
    ) {
      throw new Error(`Missing detached signature: ${path.basename(filePath)}.asc`);
    }
  }
  if (process.env.DEOX_ALLOW_UNSIGNED_RELEASE !== '1') {
    for (const checksumPath of checksumPaths) {
      if (!fs.existsSync(`${checksumPath}.asc`)) {
        throw new Error(`Missing detached signature: ${path.basename(checksumPath)}.asc`);
      }
    }
  }
  return archives;
}

function listRemoteAssets(releaseId) {
  const assets = [];
  for (let page = 1; ; page += 1) {
    const batch = githubApi(
      'GET',
      `/repos/${repository()}/releases/${releaseId}/assets?per_page=100&page=${page}`,
    );
    if (!Array.isArray(batch)) throw new Error('GitHub returned invalid asset list');
    assets.push(...batch);
    if (batch.length < 100) return assets;
  }
}

function verifyRemote(localFiles) {
  assertGitHubCliAuthenticated();
  const releases = githubApi('GET', `/repos/${repository()}/releases?per_page=100`);
  const release = releases.find((item) => item?.tag_name === tag);
  if (!release) throw new Error(`Release ${tag} not found`);
  if (!release.draft) throw new Error(`Release ${tag} is published; refusing remote mutation check`);
  const assets = new Map(listRemoteAssets(release.id).map((asset) => [asset.name, asset]));
  for (const filePath of localFiles) {
    const name = path.basename(filePath);
    const asset = assets.get(name);
    if (!asset) throw new Error(`Remote release missing ${name}`);
    if (asset.size !== fs.statSync(filePath).size) {
      throw new Error(`Remote size mismatch for ${name}`);
    }
    const expectedDigest = `sha256:${sha256(filePath)}`;
    if (asset.digest !== expectedDigest) {
      throw new Error(`Remote digest missing or mismatched for ${name}`);
    }
  }
}

function main() {
  const localFiles = verifyLocal();
  if (process.argv.includes('--remote')) {
    verifyRemote(allReleaseFiles());
  }
  console.log(`Release ${tag} verified (${localFiles.length} archive(s))`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ Release verification failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { parseChecksumManifest, verifyChecksum };
