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

/**
 * Local branches whose upstream was deleted on the remote (`[gone]`).
 * Branches that were never pushed have no upstream and are always kept, so
 * unpushed work can never be pruned.
 */
function goneBranches(refLines) {
  return refLines
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [name, ...track] = line.split(' ');
      return { name, track: track.join(' ') };
    })
    .filter(({ track }) => track === '[gone]')
    .map(({ name }) => name);
}

function requireConfirmation() {
  if (process.env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error('Refusing local branch deletion without DEOX_RELEASE_CONFIRM=YES');
  }
}

function main() {
  requireConfirmation();
  const current = run(['branch', '--show-current']);
  run(['fetch', '--prune', 'origin']);
  const refs = run([
    'for-each-ref',
    '--format=%(refname:short) %(upstream:track)',
    'refs/heads/',
  ]);
  for (const branch of goneBranches(refs)) {
    if (branch === current || branch === 'main' || branch === 'beta') continue;
    run(['branch', '-D', branch]);
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

module.exports = { requireConfirmation, goneBranches };
