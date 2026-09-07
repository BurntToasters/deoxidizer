'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const branch = process.argv[2];

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed with status ${result.status}`);
  }
}

function requireConfirmation() {
  if (process.env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error(
      'Refusing destructive branch synchronization. Set DEOX_RELEASE_CONFIRM=YES explicitly.',
    );
  }
}

function main() {
  if (!['main', 'beta'].includes(branch)) {
    throw new Error('branch must be main or beta');
  }
  requireConfirmation();
  run('git', ['fetch', 'origin']);
  run('git', ['reset', '--hard']);
  run('git', ['clean', '-fd']);
  run('git', ['switch', '-C', branch, `origin/${branch}`]);
  run('git', ['reset', '--hard', `origin/${branch}`]);
  run('git', ['clean', '-fd']);
  run('npm', ['run', 'vi']);
  if (branch === 'main') {
    run('npm', ['run', 'gitprune:force']);
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
