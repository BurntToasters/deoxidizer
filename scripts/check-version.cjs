'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const packageJson = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const cargoVersion = cargo.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
if (!cargoVersion) throw new Error('Cargo.toml has no version');
if (packageJson.version !== cargoVersion) {
  throw new Error(`package.json version ${packageJson.version} does not match Cargo.toml ${cargoVersion}`);
}
console.log(`Version metadata consistent: ${cargoVersion}`);
