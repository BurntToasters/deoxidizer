'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const {
  assertGitHubCliAuthenticated,
  githubApi,
  repository,
} = require('./github-cli.cjs');
const { cargoVersion, isPrerelease } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = cargoVersion(manifest);

const tag = `v${version}`;
const prerelease = isPrerelease(version);
const waitMode = process.argv.includes('--wait');
function envTimeoutMs(name, fallback) {
  const value = Number(process.env[name] ?? fallback);
  return Number.isFinite(value) && value > 0 ? value : fallback;
}
const waitTimeoutMs = envTimeoutMs('RELEASE_DRAFT_WAIT_TIMEOUT_MS', 30 * 60 * 1000);
const waitPollMs = envTimeoutMs('RELEASE_DRAFT_WAIT_POLL_MS', 15_000);

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

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
  // Paginate: a single per_page=100 page can miss the tag once history grows.
  // Fall back to the direct tag endpoint (works once the git tag exists).
  for (let page = 1; ; page += 1) {
    const releases = githubApi('GET', `/repos/${repository()}/releases?per_page=100&page=${page}`);
    if (!Array.isArray(releases)) throw new Error('GitHub returned invalid release list');
    const found = releases.find((release) => release?.tag_name === tag);
    if (found) return found;
    if (releases.length < 100) break;
  }
  try {
    const byTag = githubApi('GET', `/repos/${repository()}/releases/tags/${tag}`);
    if (byTag?.tag_name === tag) return byTag;
  } catch {
    // Drafts have no git tag yet, so 404 here is expected; fall through.
  }
  return null;
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

function syncNotes(release) {
  // Reuse path refreshes name/body so concurrent builders converge on
  // current CHANGELOG.md instead of keeping a stale first-writer body.
  return githubApi('PATCH', `/repos/${repository()}/releases/${release.id}`, {
    name: `deoxidizer ${version}`,
    body: releaseNotes(),
  });
}

function checkBinding(release) {
  if (!release.draft) {
    throw new Error(`Release ${tag} is already published; refusing to mutate it`);
  }
  const commit = currentCommit();
  if (release.target_commitish?.toLowerCase() !== commit.toLowerCase()) {
    throw new Error(
      `Release ${tag} is bound to ${release.target_commitish}, not current HEAD ${commit}`,
    );
  }
  return release;
}

async function ensureDraft() {
  const existing = findRelease();
  if (existing) {
    const bound = checkBinding(existing);
    return syncNotes(bound);
  }
  try {
    return githubApi('POST', `/repos/${repository()}/releases`, {
      tag_name: tag,
      target_commitish: currentCommit(),
      name: `deoxidizer ${version}`,
      body: releaseNotes(),
      draft: true,
      prerelease,
    });
  } catch (error) {
    // Another concurrent builder may have created the draft (422 validation
    // failed / already_exists): sleep 2s then re-fetch up to 3 attempts and
    // reuse it instead of failing (see iyeris ensure-draft-release.cjs).
    if (error?.statusCode === 422) {
      for (let attempt = 1; attempt <= 3; attempt += 1) {
        await sleep(2000);
        const retry = findRelease();
        if (retry) {
          return syncNotes(checkBinding(retry));
        }
      }
    }
    throw error;
  }
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
  const release = waitMode ? await waitForDraft() : await ensureDraft();
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
