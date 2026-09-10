'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');

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

module.exports = { requireConfirmation, parseArgs };
