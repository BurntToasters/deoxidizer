'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');

const root = path.resolve(__dirname, '..');

function run(args, allowFailure = false) {
  const result = spawnSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  if (result.error) throw result.error;
  if (!allowFailure && result.status !== 0) {
    throw new Error(`git ${args.join(' ')} failed with status ${result.status}`);
  }
  return String(result.stdout || '').trim();
}

function hasRemoteBranch(branch) {
  const result = spawnSync(
    'git',
    ['show-ref', '--verify', '--quiet', `refs/remotes/origin/${branch}`],
    { cwd: root, stdio: 'ignore' },
  );
  if (result.error) throw result.error;
  return result.status === 0;
}

function requireConfirmation() {
  if (process.env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error('Refusing local branch deletion without DEOX_RELEASE_CONFIRM=YES');
  }
}

function main() {
  requireConfirmation();
  const current = run(['branch', '--show-current']);
  const branches = run(['for-each-ref', '--format=%(refname:short)', 'refs/heads/'])
    .split(/\r?\n/)
    .filter(Boolean);
  for (const branch of branches) {
    if (branch === current || branch === 'main' || branch === 'beta') continue;
    if (!hasRemoteBranch(branch)) {
      run(['branch', '-D', branch]);
    }
  }
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { requireConfirmation };
