'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {
  assertGitHubCliAuthenticated,
  findReleaseByTag,
  githubApiWithRetry,
  repository,
} = require('./github-cli.cjs');
const { spawnSync } = require('node:child_process');
const { cargoVersion, isPrerelease } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = cargoVersion(manifest);
const tag = `v${version}`;
// Shared strict prerelease predicate from sync-version.cjs (mirrors
// validateVersion suffix); do not duplicate an ad-hoc regex here.
const prerelease = isPrerelease(version);

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
    names.push(archive, `${archive}.minisig`, checksum, `${checksum}.minisig`);
    if (os === 'windows') {
      const installer = `deoxidizer-v${version}-windows-${arch}-setup.exe`;
      names.push(installer, `${installer}.minisig`);
    }
  }
  return new Set(names);
}

function currentCommit() {
  const result = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  if (result.error || result.status !== 0) throw new Error('Cannot determine current Git commit');
  const commit = String(result.stdout || '').trim();
  if (!/^[0-9a-f]{40}$/i.test(commit)) throw new Error('Git returned invalid current commit');
  return commit;
}

function validCommitSha(sha) {
  if (!/^[0-9a-f]{40}$/i.test(String(sha || ''))) {
    throw new Error('Git tag does not resolve to a full commit SHA');
  }
  return String(sha).toLowerCase();
}

function peelTagCommit(refObject, fetchTagObject) {
  let object = refObject?.object || refObject;
  if (object?.type !== 'tag') {
    throw new Error('Git tag ref is not an annotated tag object that resolves to a commit');
  }
  const visited = new Set();
  for (let depth = 0; depth < 16; depth += 1) {
    const sha = validCommitSha(object.sha);
    if (visited.has(sha)) throw new Error('Git tag object cycle does not resolve to a commit');
    visited.add(sha);
    const tagged = fetchTagObject(sha);
    object = tagged?.object || tagged;
    if (object?.type === 'commit') return validCommitSha(object.sha);
    if (object?.type !== 'tag') throw new Error('Git tag does not resolve to a commit object');
  }
  throw new Error('Git tag exceeds the annotated-tag peel depth');
}

function resolveTagCommit(tagName, environment = process.env) {
  const ref = githubApiWithRetry(
    'GET',
    `/repos/${repository()}/git/ref/tags/${encodeURIComponent(tagName)}`,
    undefined,
    { environment },
  );
  const object = ref?.object;
  if (object?.type === 'commit') return validCommitSha(object.sha);
  if (object?.type !== 'tag') throw new Error(`Git tag ${tagName} has no commit or annotated-tag ref`);
  return peelTagCommit(object, (sha) =>
    githubApiWithRetry('GET', `/repos/${repository()}/git/tags/${sha}`, undefined, { environment }),
  );
}

function validateDraft(release, assets, expectedCommit, tagCommit) {
  const errors = [];
  if (!release?.draft) errors.push(`${tag} is not a draft release`);
  if (release?.tag_name !== tag) errors.push(`expected tag ${tag}`);
  if (release?.prerelease !== prerelease) {
    errors.push(`prerelease flag does not match ${version}`);
  }
  if (expectedCommit && release?.target_commitish?.toLowerCase() !== expectedCommit.toLowerCase()) {
    errors.push(`target commit does not match current HEAD ${expectedCommit}`);
  }
  if (expectedCommit && tagCommit && tagCommit.toLowerCase() !== expectedCommit.toLowerCase()) {
    errors.push(`tag commit does not match current HEAD ${expectedCommit}`);
  }
  const expected = expectedReleaseAssets();
  const byName = new Map();
  for (const asset of assets) {
    if (!asset?.name) continue;
    if (byName.has(asset.name)) errors.push(`duplicate asset: ${asset.name}`);
    byName.set(asset.name, asset);
    if (!Number.isFinite(asset.size) || asset.size <= 0) {
      errors.push(`empty asset: ${asset.name}`);
    }
  }
  for (const name of expected) {
    if (!byName.has(name)) errors.push(`missing asset: ${name}`);
  }
  for (const name of byName.keys()) {
    if (!expected.has(name)) errors.push(`unexpected asset: ${name}`);
  }
  return errors;
}

function findDraft() {
  // Shared paginated lookup with tag-endpoint fallback (see github-cli.cjs).
  // Binding checks stay in validateDraft/verifyDraft via target_commitish.
  return findReleaseByTag(tag);
}

function listAssets(releaseId) {
  const assets = [];
  for (let page = 1; ; page += 1) {
    const batch = githubApiWithRetry(
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
  const commit = currentCommit();
  const tagCommit = resolveTagCommit(tag);
  const errors = validateDraft(release, listAssets(release.id), commit, tagCommit);
  if (errors.length > 0) throw new Error(errors.join('\n'));
  return release;
}

function verifyDraftIntegrity() {
  const result = spawnSync(
    process.execPath,
    [path.join(__dirname, 'verify-release.cjs'), '--remote-complete'],
    {
      cwd: root,
      encoding: 'utf8',
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr, result.stdout].filter(Boolean).join('\n').trim();
    throw new Error(detail || `Remote integrity verification failed with status ${result.status}`);
  }
  return String(result.stdout || '').trim();
}

if (require.main === module) {
  try {
    if (process.argv.includes('--integrity')) {
      verifyDraftIntegrity();
      console.log(`Release draft ${tag} passed complete remote integrity verification`);
    } else {
      assertGitHubCliAuthenticated();
      const release = verifyDraft();
      console.log(`Release draft ${release.tag_name} is complete`);
    }
  } catch (error) {
    console.error(`✗ Draft verification failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = {
  TARGETS,
  expectedReleaseAssets,
  validateDraft,
  verifyDraft,
  verifyDraftIntegrity,
  tag,
  prerelease,
  currentCommit,
  peelTagCommit,
  resolveTagCommit,
};
