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
const waitMode = process.argv.includes('--wait');
const waitTimeoutMs = Number(process.env.RELEASE_DRAFT_WAIT_TIMEOUT_MS || 30 * 60 * 1000);
const waitPollMs = Number(process.env.RELEASE_DRAFT_WAIT_POLL_MS || 15_000);

function releaseNotes() {
  const changelog = path.join(root, 'CHANGELOG.md');
  if (!fs.existsSync(changelog)) {
    throw new Error('CHANGELOG.md is required for release notes');
  }
  const body = fs.readFileSync(changelog, 'utf8').trim();
  if (!body) throw new Error('CHANGELOG.md is empty');
  return body;
}

function findRelease() {
  const releases = githubApi('GET', `/repos/${repository()}/releases?per_page=100`);
  if (!Array.isArray(releases)) throw new Error('GitHub returned invalid release list');
  return releases.find((release) => release?.tag_name === tag) || null;
}

function ensureDraft() {
  const existing = findRelease();
  if (existing) {
    if (!existing.draft) {
      throw new Error(`Release ${tag} is already published; refusing to mutate it`);
    }
    return existing;
  }
  return githubApi('POST', `/repos/${repository()}/releases`, {
    tag_name: tag,
    name: `deoxidizer ${version}`,
    body: releaseNotes(),
    draft: true,
    prerelease,
  });
}

async function waitForDraft() {
  const deadline = Date.now() + waitTimeoutMs;
  while (Date.now() < deadline) {
    const release = findRelease();
    if (release) {
      if (!release.draft) throw new Error(`Release ${tag} is already published`);
      return release;
    }
    await new Promise((resolve) => setTimeout(resolve, waitPollMs));
  }
  throw new Error(`Timed out waiting for draft release ${tag}`);
}

async function main() {
  assertGitHubCliAuthenticated();
  const release = waitMode ? await waitForDraft() : ensureDraft();
  console.log(
    `Draft ${tag} ready (${release.id}, ${release.assets?.length || 0} assets) in ${repository()}`,
  );
}

if (require.main === module) {
  main().catch((error) => {
    console.error(`✗ Failed to ensure draft release: ${error?.message || error}`);
    process.exit(1);
  });
}

module.exports = { tag, version, prerelease, releaseNotes };
