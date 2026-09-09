'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
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

function currentCommit() {
  const result = spawnSync('git', ['rev-parse', 'HEAD'], {
    cwd: root,
    encoding: 'utf8',
  });
  if (result.error || result.status !== 0) {
    throw new Error('Cannot determine current Git commit for release binding');
  }
  const commit = String(result.stdout || '').trim();
  if (!/^[0-9a-f]{40}$/i.test(commit)) {
    throw new Error('Git returned invalid current commit for release binding');
  }
  return commit;
}

function ensureDraft() {
  const existing = findRelease();
  if (existing) {
    if (!existing.draft) {
      throw new Error(`Release ${tag} is already published; refusing to mutate it`);
    }
    const commit = currentCommit();
    if (existing.target_commitish?.toLowerCase() !== commit.toLowerCase()) {
      throw new Error(
        `Release ${tag} is bound to ${existing.target_commitish}, not current HEAD ${commit}`,
      );
    }
    return existing;
  }
  return githubApi('POST', `/repos/${repository()}/releases`, {
    tag_name: tag,
    target_commitish: currentCommit(),
    name: `deoxidizer ${version}`,
    body: releaseNotes(),
    draft: true,
    prerelease,
  });
}

async function waitForDraft() {
  const deadline = Date.now() + waitTimeoutMs;
  const commit = currentCommit();
  while (Date.now() < deadline) {
    const release = findRelease();
    if (release) {
      if (!release.draft) throw new Error(`Release ${tag} is already published`);
      if (release.target_commitish?.toLowerCase() !== commit.toLowerCase()) {
        throw new Error(
          `Release ${tag} is bound to ${release.target_commitish || '<missing>'}, not current HEAD ${commit}`,
        );
      }
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
