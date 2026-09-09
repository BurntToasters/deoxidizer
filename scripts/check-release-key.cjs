'use strict';

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const expectedFingerprint = 'CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F';

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), 'utf8');
}

function main() {
  const output = execFileSync(
    'gpg',
    ['--batch', '--show-keys', '--with-colons', path.join(root, 'release-signing-key.asc')],
    { cwd: root, encoding: 'utf8' },
  );
  const fingerprint = output
    .split(/\r?\n/)
    .map((line) => line.split(':'))
    .find((fields) => fields[0] === 'fpr')?.[9]
    ?.toUpperCase();
  if (fingerprint !== expectedFingerprint) {
    throw new Error(
      `release-signing-key.asc fingerprint ${fingerprint || '<missing>'} does not match ${expectedFingerprint}`,
    );
  }

  for (const relativePath of ['install.sh', 'install.ps1']) {
    if (!read(relativePath).toUpperCase().includes(expectedFingerprint)) {
      throw new Error(`${relativePath} does not pin the release key fingerprint`);
    }
  }
  console.log(`Release signing key consistent: ${expectedFingerprint}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Release key check failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { expectedFingerprint, main };
