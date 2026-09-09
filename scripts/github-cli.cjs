'use strict';

const { spawnSync } = require('node:child_process');

function githubCliEnvironment(environment = process.env) {
  const childEnvironment = { ...environment };
  delete childEnvironment.GH_TOKEN;
  delete childEnvironment.GITHUB_TOKEN;
  return childEnvironment;
}

function githubStatusCode(detail) {
  const match = String(detail || '').match(/\bHTTP\s+(\d{3})\b|\bstatus(?: code)?\s+(\d{3})\b/i);
  return match ? Number(match[1] || match[2]) : undefined;
}

function runGitHub(args, { input, allowFailure = false, environment = process.env } = {}) {
  const result = spawnSync('gh', args, {
    cwd: process.cwd(),
    encoding: 'utf8',
    env: githubCliEnvironment(environment),
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

function assertGitHubCliAuthenticated() {
  runGitHub(['auth', 'status', '--hostname', 'github.com']);
}

function repository() {
  const owner = process.env.GH_REPO_OWNER || 'BurntToasters';
  const name = process.env.GH_REPO_NAME || 'deoxidizer';
  return `${owner}/${name}`;
}

function githubApi(method, endpoint, body) {
  const args = ['api', '--method', method, endpoint];
  const options = {};
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
    // Atomics.wait unavailable; fall back to blocking sleep.
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

// Bounded retry for transient GitHub API failures (408/429/5xx).
// Retries up to 3 times with exponential backoff 1s/2s/4s, honoring a
// Retry-After hint when the gh error detail carries one. Non-retryable
// errors (including 422 create races, handled separately by callers)
// throw immediately. Single-attempt githubApi/runGitHub remain for
// non-idempotent or already-guarded paths.
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

function githubApiWithRetry(method, endpoint, body) {
  return withGitHubRetry(() => githubApi(method, endpoint, body));
}

// Shared release lookup: paginate the releases list (a single
// per_page=100 page can miss the tag once history grows), then fall back
// to the direct tag endpoint (works once the git tag exists; 404 is
// expected for tag-less drafts). Callers keep their own draft/binding
// checks; this helper only finds by tag.
function findReleaseByTag(tag) {
  for (let page = 1; ; page += 1) {
    const releases = githubApiWithRetry(
      'GET',
      `/repos/${repository()}/releases?per_page=100&page=${page}`,
    );
    if (!Array.isArray(releases)) throw new Error('GitHub returned invalid release list');
    const found = releases.find((release) => release?.tag_name === tag);
    if (found) return found;
    if (releases.length < 100) break;
  }
  try {
    const byTag = githubApiWithRetry('GET', `/repos/${repository()}/releases/tags/${tag}`);
    if (byTag?.tag_name === tag) return byTag;
  } catch {
    // Drafts have no git tag yet, so 404 here is expected; fall through.
  }
  return null;
}

function uploadReleaseAsset(tag, filePath, { clobber = false, environment = process.env } = {}) {
  const args = ['release', 'upload', tag, '--repo', repository()];
  if (clobber) args.push('--clobber');
  args.push(filePath);
  // Uploads are idempotent with --clobber, so transient 408/429/5xx are
  // safe to retry with the same bounded backoff as API lookups.
  return withGitHubRetry(() => runGitHub(args, { environment }));
}

module.exports = {
  assertGitHubCliAuthenticated,
  findReleaseByTag,
  githubApi,
  githubApiWithRetry,
  githubCliEnvironment,
  githubStatusCode,
  isRetryableGitHubStatus,
  repository,
  runGitHub,
  uploadReleaseAsset,
  withGitHubRetry,
};
