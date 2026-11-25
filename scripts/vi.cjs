'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { npmInvocation } = require('./npm-safe-update.cjs');

const root = path.resolve(__dirname, '..');

function run(command, args, { cwd = root, env = process.env } = {}) {
  const result = spawnSync(command, args, {
    cwd,
    env,
    encoding: 'utf8',
    shell: false,
    windowsHide: true,
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed with status ${result.status}`);
  }
}

function requireUpstream({ cwd = root, env = process.env } = {}) {
  const upstreamResult = spawnSync(
    'git',
    ['rev-parse', '--symbolic-full-name', '--verify', '@{u}'],
    { cwd, env, encoding: 'utf8', shell: false, windowsHide: true, stdio: 'pipe' },
  );
  if (upstreamResult.error) throw upstreamResult.error;
  const ref = String(upstreamResult.stdout || '').trim();
  const prefix = 'refs/remotes/';
  if (upstreamResult.status !== 0 || !ref.startsWith(prefix)) {
    throw new Error('No valid remote tracking branch (@{u}); refusing to reset or clean');
  }

  const remoteAndBranch = ref.slice(prefix.length);
  const separator = remoteAndBranch.indexOf('/');
  if (separator <= 0 || separator === remoteAndBranch.length - 1) {
    throw new Error(`Invalid upstream ref ${ref}; refusing to reset or clean`);
  }
  const remote = remoteAndBranch.slice(0, separator);
  const branch = remoteAndBranch.slice(separator + 1);
  const remoteResult = spawnSync('git', ['ls-remote', '--exit-code', '--heads', remote, branch], {
    cwd,
    env,
    encoding: 'utf8',
    shell: false,
    windowsHide: true,
    stdio: 'pipe',
  });
  if (remoteResult.error) throw remoteResult.error;
  const exactRemoteRef = `refs/heads/${branch}`;
  const hasExactBranch = String(remoteResult.stdout || '')
    .split(/\r?\n/)
    .some((line) => line.trim().split(/\s+/)[1] === exactRemoteRef);
  if (remoteResult.status !== 0 || !hasExactBranch) {
    throw new Error(`Upstream branch ${remote}/${branch} is missing or unreachable; refusing to reset or clean`);
  }
  return `${remote}/${branch}`;
}

function requireConfirmation(env = process.env) {
  if (env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error(
      'Refusing destructive Git synchronization. Set DEOX_RELEASE_CONFIRM=YES explicitly.',
    );
  }
}

function main() {
  const env = process.env;
  requireConfirmation(env);
  const npm = npmInvocation({ env, platform: process.platform, execPath: process.execPath });
  run('git', ['fetch', 'origin'], { env });
  requireUpstream({ env });
  run('git', ['reset', '--hard'], { env });
  run('git', ['clean', '-fd'], { env });
  run('git', ['reset', '--hard', '@{u}'], { env });
  run('git', ['pull', '--ff-only'], { env });
  run(npm.command, [...npm.prefixArgs, 'ci', '--ignore-scripts'], { env });
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { requireConfirmation, requireUpstream };
