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

function uploadReleaseAsset(tag, filePath, { clobber = false, environment = process.env } = {}) {
  const args = ['release', 'upload', tag, '--repo', repository()];
  if (clobber) args.push('--clobber');
  args.push(filePath);
  runGitHub(args, { environment });
}

module.exports = {
  assertGitHubCliAuthenticated,
  githubApi,
  githubCliEnvironment,
  githubStatusCode,
  repository,
  runGitHub,
  uploadReleaseAsset,
};
