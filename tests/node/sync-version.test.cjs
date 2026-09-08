'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const {
  updateCargoLock,
  updateCargoToml,
  updateChangelog,
  updatePackageLock,
  validateVersion,
} = require('../../scripts/sync-version.cjs');

const root = path.resolve(__dirname, '../..');

test('accepts stable and beta release versions', () => {
  assert.equal(validateVersion('0.1.1'), '0.1.1');
  assert.equal(validateVersion('0.2.0-beta.1'), '0.2.0-beta.1');
  assert.throws(() => validateVersion('0.1'), /Invalid release version/);
});

test('updates only root Cargo package version', () => {
  const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  const updated = updateCargoToml(cargo, '0.1.1');
  assert.match(updated, /^\[package\][\s\S]*^version = "0\.1\.1"/m);
  assert.match(updated, /rust-version = "1\.98"/);
});

test('updates Cargo.lock and npm lock root package versions', () => {
  const cargoLock = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
  const packageLock = fs.readFileSync(path.join(root, 'package-lock.json'), 'utf8');
  const updatedCargoLock = updateCargoLock(cargoLock, '0.1.1');
  const updatedPackageLock = JSON.parse(updatePackageLock(packageLock, '0.1.1'));

  assert.match(updatedCargoLock, /name = "deoxidizer"\nversion = "0\.1\.1"/);
  assert.equal(updatedPackageLock.version, '0.1.1');
  assert.equal(updatedPackageLock.packages[''].version, '0.1.1');
});

test('updates BCLS heading and download tag', () => {
  const changelog = fs.readFileSync(path.join(root, 'CHANGELOG.md'), 'utf8');
  const updated = updateChangelog(changelog, '0.1.0', '0.1.1');
  assert.match(updated, /## Changes in `v0\.1\.1:`/);
  assert.match(updated, /releases\/download\/v0\.1\.1\//);
  assert.doesNotMatch(updated, /releases\/download\/v0\.1\.0\//);
});
