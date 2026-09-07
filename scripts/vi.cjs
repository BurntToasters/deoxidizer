'use strict';

const { spawnSync } = require('node:child_process');
const path = require('node:path');

const root = path.resolve(__dirname, '..');

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
      'Refusing destructive Git synchronization. Set DEOX_RELEASE_CONFIRM=YES explicitly.',
    );
  }
}

function main() {
  requireConfirmation();
  run('git', ['fetch', 'origin']);
  run('git', ['reset', '--hard']);
  run('git', ['clean', '-fd']);
  run('git', ['reset', '--hard', '@{u}']);
  run('git', ['pull', '--ff-only']);
  run('npm', ['ci', '--ignore-scripts']);
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
