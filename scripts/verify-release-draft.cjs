'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {
  assertGitHubCliAuthenticated,
  githubApi,
  repository,
} = require('./github-cli.cjs');

const root = path.resolve(__dirname, '..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error('Cargo.toml has no version');
const tag = `v${version}`;
const prerelease = /-(?:alpha|beta|rc)(?:[.-]?\d+)?$/i.test(version);

const TARGETS = [
  ['linux', 'x86_64', 'tar.gz'],
  ['linux', 'aarch64', 'tar.gz'],
  ['darwin', 'x86_64', 'tar.gz'],
  ['darwin', 'aarch64', 'tar.gz'],
  ['windows', 'x86_64', 'zip'],
  ['windows', 'aarch64', 'zip'],
];

function expectedReleaseAssets() {
  const names = [];
  for (const [os, arch, extension] of TARGETS) {
    const archive = `deoxidizer-v${version}-${os}-${arch}.${extension}`;
    const checksum = `SHA256SUMS-${os}-${arch}.txt`;
    names.push(archive, `${archive}.asc`, checksum, `${checksum}.asc`);
    if (os === 'windows') {
      const installer = `deoxidizer-v${version}-windows-${arch}-setup.exe`;
      names.push(installer, `${installer}.asc`);
    }
  }
  return new Set(names);
}

function validateDraft(release, assets) {
  const errors = [];
  if (!release?.draft) errors.push(`${tag} is not a draft release`);
  if (release?.tag_name !== tag) errors.push(`expected tag ${tag}`);
  if (release?.prerelease !== prerelease) {
    errors.push(`prerelease flag does not match ${version}`);
  }
  const byName = new Map();
  for (const asset of assets) {
    if (!asset?.name) continue;
    if (byName.has(asset.name)) errors.push(`duplicate asset: ${asset.name}`);
    byName.set(asset.name, asset);
    if (!Number.isFinite(asset.size) || asset.size <= 0) {
      errors.push(`empty asset: ${asset.name}`);
    }
  }
  for (const name of expectedReleaseAssets()) {
    if (!byName.has(name)) errors.push(`missing asset: ${name}`);
  }
  return errors;
}

function findDraft() {
  const releases = githubApi('GET', `/repos/${repository()}/releases?per_page=100`);
  if (!Array.isArray(releases)) throw new Error('GitHub returned invalid releases payload');
  return releases.find((release) => release?.tag_name === tag) || null;
}

function listAssets(releaseId) {
  const assets = [];
  for (let page = 1; ; page += 1) {
    const batch = githubApi(
      'GET',
      `/repos/${repository()}/releases/${releaseId}/assets?per_page=100&page=${page}`,
    );
    if (!Array.isArray(batch)) throw new Error('GitHub returned invalid assets payload');
    assets.push(...batch);
    if (batch.length < 100) return assets;
  }
}

function verifyDraft() {
  const release = findDraft();
  if (!release) throw new Error(`No release found for ${tag}`);
  const errors = validateDraft(release, listAssets(release.id));
  if (errors.length > 0) throw new Error(errors.join('\n'));
  return release;
}

if (require.main === module) {
  try {
    assertGitHubCliAuthenticated();
    const release = verifyDraft();
    console.log(`Release draft ${release.tag_name} is complete`);
  } catch (error) {
    console.error(`✗ Draft verification failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { TARGETS, expectedReleaseAssets, validateDraft, verifyDraft, tag, prerelease };
