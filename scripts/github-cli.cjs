'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

function githubCliEnvironment(environment = process.env, { allowEnvironmentToken = false } = {}) {
  const childEnvironment = { ...environment };
  if (!allowEnvironmentToken) {
    delete childEnvironment.GH_TOKEN;
    delete childEnvironment.GITHUB_TOKEN;
  }
  return childEnvironment;
}

function githubStatusCode(detail) {
  const match = String(detail || '').match(/\bHTTP\s+(\d{3})\b|\bstatus(?: code)?\s+(\d{3})\b/i);
  return match ? Number(match[1] || match[2]) : undefined;
}

function runGitHub(
  args,
  { input, allowFailure = false, environment = process.env, allowEnvironmentToken = false } = {},
) {
  const result = spawnSync('gh', args, {
    cwd: process.cwd(),
    encoding: 'utf8',
    env: githubCliEnvironment(environment, { allowEnvironmentToken }),
    input,
    stdio: ['pipe', 'pipe', 'pipe'],
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) {
    if (result.error.code === 'ENOENT') {
      throw new Error('GitHub CLI is required. Install gh and run `gh auth login`.');
    }
    throw result.error;
  }
  if (result.status !== 0 && !allowFailure) {
    const detail = [result.stderr, result.stdout].filter(Boolean).join('\n').trim();
    const error = new Error(
      `gh ${args.join(' ')} failed with status ${result.status}${detail ? `:\n${detail}` : ''}`,
    );
    error.statusCode = githubStatusCode(detail);
    throw error;
  }
  return result;
}

function githubOutput(args, options) {
  return String(runGitHub(args, options).stdout || '').trim();
}

function githubJson(args, options) {
  const output = githubOutput(args, options);
  return output ? JSON.parse(output) : {};
}

function assertGitHubCliAuthenticated(environment = process.env) {
  runGitHub(['auth', 'status', '--hostname', 'github.com'], {
    environment,
    allowEnvironmentToken: true,
  });
}

function repository() {
  const owner = process.env.GH_REPO_OWNER || 'BurntToasters';
  const name = process.env.GH_REPO_NAME || 'deoxidizer';
  return `${owner}/${name}`;
}

function githubApi(method, endpoint, body, { environment = process.env } = {}) {
  const args = ['api', '--method', method, endpoint];
  const options = { environment, allowEnvironmentToken: true };
  if (body !== undefined) {
    args.push('--input', '-');
    options.input = JSON.stringify(body);
  }
  return githubJson(args, options);
}

function sleepSync(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return;
  try {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
  } catch {
    spawnSync('sleep', [String(ms / 1000)], { stdio: 'ignore' });
  }
}

function isRetryableGitHubStatus(statusCode) {
  return (
    statusCode === 408 ||
    statusCode === 429 ||
    (Number.isInteger(statusCode) && statusCode >= 500 && statusCode <= 599)
  );
}

function retryAfterMs(detail, attempt) {
  const match = String(detail || '').match(/retry-after[:\s]+(\d+)/i);
  if (match) {
    const seconds = Number(match[1]);
    if (Number.isFinite(seconds) && seconds >= 0 && seconds <= 300) {
      return seconds * 1000;
    }
  }
  return [1000, 2000, 4000][Math.min(attempt, 2)] ?? 4000;
}

// Bounded retries are safe for lookups and immutable asset creation. Asset
// replacement is never retried or enabled.
function withGitHubRetry(fn, { maxRetries = 3 } = {}) {
  let lastError;
  for (let attempt = 0; attempt <= maxRetries; attempt += 1) {
    try {
      return fn();
    } catch (error) {
      lastError = error;
      const status = error?.statusCode ?? githubStatusCode(error?.message);
      if (attempt >= maxRetries || !isRetryableGitHubStatus(status)) throw error;
      sleepSync(retryAfterMs(error?.message, attempt));
    }
  }
  throw lastError;
}

function githubApiWithRetry(method, endpoint, body, options) {
  return withGitHubRetry(() => githubApi(method, endpoint, body, options));
}

// Drafts have no tag yet, so the direct tag endpoint is a fallback after the
// paginated release listing.
function findReleaseByTag(tag, { environment = process.env } = {}) {
  for (let page = 1; ; page += 1) {
    const releases = githubApiWithRetry(
      'GET',
      `/repos/${repository()}/releases?per_page=100&page=${page}`,
      undefined,
      { environment },
    );
    if (!Array.isArray(releases)) throw new Error('GitHub returned invalid release list');
    const found = releases.find((release) => release?.tag_name === tag);
    if (found) return found;
    if (releases.length < 100) break;
  }
  try {
    const byTag = githubApiWithRetry(
      'GET',
      `/repos/${repository()}/releases/tags/${encodeURIComponent(tag)}`,
      undefined,
      { environment },
    );
    if (byTag?.tag_name === tag) return byTag;
  } catch {
    // A draft can exist before the tag ref is available.
  }
  return null;
}

function sha256File(filePath) {
  const descriptor = fs.openSync(filePath, 'r');
  const hash = crypto.createHash('sha256');
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    while (true) {
      const read = fs.readSync(descriptor, buffer, 0, buffer.length, null);
      if (read === 0) break;
      hash.update(buffer.subarray(0, read));
    }
  } finally {
    fs.closeSync(descriptor);
  }
  return `sha256:${hash.digest('hex')}`;
}

function validateExistingReleaseAsset(asset, expected) {
  if (!asset || asset.name !== expected.name) {
    throw new Error(`Existing release asset name does not match ${expected.name}`);
  }
  if (asset.size !== expected.size) {
    throw new Error(`Existing release asset ${expected.name} is immutable and has a different size`);
  }
  if (String(asset.digest || '').toLowerCase() !== expected.digest.toLowerCase()) {
    throw new Error(
      `Existing release asset ${expected.name} is immutable and has a different or unavailable digest`,
    );
  }
  return true;
}

function listReleaseAssets(releaseId, environment = process.env) {
  const assets = [];
  for (let page = 1; ; page += 1) {
    const batch = githubApiWithRetry(
      'GET',
      `/repos/${repository()}/releases/${releaseId}/assets?per_page=100&page=${page}`,
      undefined,
      { environment },
    );
    if (!Array.isArray(batch)) throw new Error('GitHub returned invalid release asset list');
    assets.push(...batch);
    if (batch.length < 100) return assets;
  }
}

function existingReleaseAsset(tag, name, environment = process.env) {
  const release = findReleaseByTag(tag, { environment });
  if (!release || !release.draft) {
    throw new Error(`A draft release for ${tag} is required before uploading assets`);
  }
  const matches = listReleaseAssets(release.id, environment).filter((asset) => asset.name === name);
  if (matches.length > 1) throw new Error(`Remote release has duplicate asset ${name}`);
  return matches[0] || null;
}

function uploadReleaseAsset(tag, filePath, { clobber = false, environment = process.env } = {}) {
  if (clobber) throw new Error('Release asset replacement is disabled; existing assets are immutable');
  const stats = fs.statSync(filePath);
  if (!stats.isFile() || stats.size <= 0) throw new Error(`Invalid staged release asset: ${filePath}`);
  const expected = {
    name: path.basename(filePath),
    size: stats.size,
    digest: sha256File(filePath),
  };
  const existing = existingReleaseAsset(tag, expected.name, environment);
  if (existing) {
    validateExistingReleaseAsset(existing, expected);
    return existing;
  }

  const args = ['release', 'upload', tag, '--repo', repository(), filePath];
  try {
    return withGitHubRetry(() => runGitHub(args, { environment, allowEnvironmentToken: true }));
  } catch (error) {
    // Builders may race on the same missing asset. Accept only an exact-byte
    // winner; never delete or replace its asset.
    const raced = existingReleaseAsset(tag, expected.name, environment);
    if (raced) {
      validateExistingReleaseAsset(raced, expected);
      return raced;
    }
    throw error;
  }
}

function releaseMutationLockRef(tag) {
  if (!/^v\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+)?$/.test(String(tag))) {
    throw new Error(`Invalid release tag for mutation lock: ${tag}`);
  }
  return `refs/heads/deoxidizer-release-lock-${tag}`;
}

function withReleaseMutationLock(tag, commit, operation, { environment = process.env } = {}) {
  if (typeof operation !== 'function') throw new Error('Release mutation lock requires an operation');
  if (!/^[0-9a-f]{40}$/i.test(String(commit || ''))) {
    throw new Error('Release mutation lock requires the full candidate commit SHA');
  }
  const ref = releaseMutationLockRef(tag);
  const refName = ref.slice('refs/'.length);
  try {
    githubApi('POST', `/repos/${repository()}/git/refs`, { ref, sha: commit }, { environment });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    if (/422|reference already exists|already exists/i.test(detail)) {
      throw new Error(
        `Release mutation lock ${ref} is already held; finish or inspect that release before retrying`,
      );
    }
    throw error;
  }

  let value;
  let operationError;
  try {
    value = operation();
  } catch (error) {
    operationError = error;
  }

  let unlockError;
  try {
    const current = githubApi('GET', `/repos/${repository()}/git/ref/${refName}`, undefined, { environment });
    if (String(current?.object?.sha || '').toLowerCase() !== commit.toLowerCase()) {
      throw new Error(`Release mutation lock ${ref} changed owner; refusing to delete it`);
    }
    githubApi('DELETE', `/repos/${repository()}/git/${refName}`, undefined, { environment });
  } catch (error) {
    unlockError = error;
  }
  if (operationError && unlockError) {
    throw new AggregateError([operationError, unlockError], 'Release operation failed and its lock could not be removed');
  }
  if (operationError) throw operationError;
  if (unlockError) throw unlockError;
  return value;
}

function downloadGitHubAsset(assetId, destination, { environment = process.env, maxBytes } = {}) {
  if (!Number.isInteger(assetId) || assetId <= 0) throw new Error('Remote release asset has no valid ID');
  if (!Number.isInteger(maxBytes) || maxBytes <= 0) throw new Error('Remote asset download limit is invalid');
  const result = spawnSync(
    'gh',
    [
      'api',
      '-H',
      'Accept: application/octet-stream',
      `/repos/${repository()}/releases/assets/${assetId}`,
    ],
    {
      cwd: process.cwd(),
      env: githubCliEnvironment(environment, { allowEnvironmentToken: true }),
      encoding: null,
      stdio: ['ignore', 'pipe', 'pipe'],
      maxBuffer: maxBytes + 1,
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = [result.stderr, result.stdout]
      .filter(Boolean)
      .map((value) => Buffer.from(value).toString('utf8'))
      .join('\n')
      .trim();
    const error = new Error(
      `gh api asset download failed with status ${result.status}${detail ? `:\n${detail}` : ''}`,
    );
    error.statusCode = githubStatusCode(detail);
    throw error;
  }
  const bytes = Buffer.from(result.stdout || []);
  if (bytes.length > maxBytes) {
    throw new Error(`Remote asset exceeds download limit: ${path.basename(destination)}`);
  }
  fs.writeFileSync(destination, bytes, { flag: 'wx', mode: 0o600 });
  return bytes.length;
}

module.exports = {
  assertGitHubCliAuthenticated,
  downloadGitHubAsset,
  existingReleaseAsset,
  findReleaseByTag,
  githubApi,
  githubApiWithRetry,
  githubCliEnvironment,
  githubStatusCode,
  isRetryableGitHubStatus,
  listReleaseAssets,
  repository,
  releaseMutationLockRef,
  runGitHub,
  sha256File,
  uploadReleaseAsset,
  validateExistingReleaseAsset,
  withReleaseMutationLock,
  withGitHubRetry,
};
