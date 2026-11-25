'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { npmInvocation } = require('./npm-safe-update.cjs');

const root = path.resolve(__dirname, '..');

function parseArgs(argv = process.argv.slice(2)) {
  const args = [...argv];
  for (const arg of args) {
    if (arg.startsWith('--') && arg !== '--force-always') {
      throw new Error(`unknown flag: ${arg} (supported: --force-always)`);
    }
  }
  return {
    branch: args.find((arg) => !arg.startsWith('--')),
    forceAlways: args.includes('--force-always'),
  };
}

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

function requireConfirmation(env = process.env) {
  if (env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error(
      'Refusing destructive branch synchronization. Set DEOX_RELEASE_CONFIRM=YES explicitly.',
    );
  }
}

function requireRemoteBranch(branch, { cwd = root, env = process.env } = {}) {
  const ref = `refs/remotes/origin/${branch}^{commit}`;
  const result = spawnSync('git', ['rev-parse', '--verify', '--quiet', ref], {
    cwd,
    env,
    encoding: 'utf8',
    shell: false,
    windowsHide: true,
    stdio: 'pipe',
  });
  if (result.error) throw result.error;
  const commit = String(result.stdout || '').trim();
  if (result.status !== 0 || !/^[0-9a-f]{40}$/i.test(commit)) {
    throw new Error(
      `Remote branch origin/${branch} is missing or does not resolve to a commit; refusing to reset or clean`,
    );
  }
  return commit;
}

function synchronizeBranch(branch, { cwd = root, env = process.env, execute = run } = {}) {
  if (!['main', 'beta'].includes(branch)) {
    throw new Error('branch must be main or beta');
  }
  const childEnv = { ...process.env, ...env };
  requireConfirmation(childEnv);

  execute('git', ['fetch', 'origin'], { cwd, env: childEnv });
  // Check the requested branch while the worktree is still untouched. A bad
  // branch name or missing origin ref must never reach reset/clean.
  requireRemoteBranch(branch, { cwd, env: childEnv });
  // Resolve npm through npm_execpath on Windows before destructive cleanup.
  // npm.cmd needs a shell, which is intentionally avoided by these scripts.
  const npm = npmInvocation({ env: childEnv, platform: process.platform, execPath: process.execPath });
  const runNpm = (args) => execute(npm.command, [...npm.prefixArgs, ...args], { cwd, env: childEnv });

  execute('git', ['reset', '--hard'], { cwd, env: childEnv });
  execute('git', ['clean', '-fd'], { cwd, env: childEnv });
  execute('git', ['switch', '-C', branch, `origin/${branch}`], { cwd, env: childEnv });
  execute('git', ['reset', '--hard', `origin/${branch}`], { cwd, env: childEnv });
  execute('git', ['clean', '-fd'], { cwd, env: childEnv });
  runNpm(['run', 'vi']);
  if (branch === 'main') runNpm(['run', 'gitprune:force']);
}

function main() {
  const { branch, forceAlways } = parseArgs();
  if (!['main', 'beta'].includes(branch)) {
    throw new Error('branch must be main or beta');
  }
  if (forceAlways) {
    console.error(
      '! --force-always passed: DEOX_RELEASE_CONFIRM gate bypassed. Destructive sync proceeds.',
    );
    process.env.DEOX_RELEASE_CONFIRM = 'YES';
  }
  synchronizeBranch(branch);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { requireConfirmation, requireRemoteBranch, synchronizeBranch, parseArgs };
