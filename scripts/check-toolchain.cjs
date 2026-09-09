'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), 'utf8');
}

function requiredMatch(text, pattern, label) {
  if (!pattern.test(text)) throw new Error(`${label} does not use the pinned Rust toolchain`);
}

function main() {
  const toolchain = read('rust-toolchain.toml').match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  const cargoVersion = read('Cargo.toml').match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];
  if (!toolchain || !cargoVersion) throw new Error('Rust toolchain declaration is incomplete');

  const numericChannel = toolchain.match(/^\d+\.\d+(?:\.\d+)?$/)?.[0];
  if (numericChannel && !toolchain.startsWith(`${cargoVersion}.`)) {
    throw new Error(`Cargo rust-version ${cargoVersion} does not match channel ${toolchain}`);
  }

  requiredMatch(
    read('.github/workflows/ci.yml'),
    new RegExp(`toolchain:\\s*${toolchain.replaceAll('.', '\\.')}`),
    '.github/workflows/ci.yml',
  );
  requiredMatch(
    read('.github/workflows/release.yml'),
    new RegExp(`toolchain:\\s*${toolchain.replaceAll('.', '\\.')}`),
    '.github/workflows/release.yml',
  );
  requiredMatch(
    read('scripts/release.cjs'),
    new RegExp(`--toolchain',\\s*'${toolchain.replaceAll('.', '\\.')}'`),
    'scripts/release.cjs',
  );
  console.log(`Rust toolchain metadata consistent: ${toolchain}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Toolchain check failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { main };
