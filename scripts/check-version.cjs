'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
// Shared [package]-scoped parser from sync-version.cjs; fallback keeps
// isolated fixture execution (staged without sync-version.cjs) runnable.
let cargoVersion;
try {
  ({ cargoVersion } = require('./sync-version.cjs'));
} catch {
  cargoVersion = (cargo) =>
    cargo.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1] ||
    (() => {
      throw new Error('Cargo.toml has no package version');
    })();
}
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const packageJson = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const version = cargoVersion(cargo);
if (packageJson.version !== version) {
  throw new Error(`package.json version ${packageJson.version} does not match Cargo.toml ${version}`);
}
console.log(`Version metadata consistent: ${version}`);
