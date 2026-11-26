'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const root = path.resolve(__dirname, '../..');
const rehearsal = path.join(root, 'scripts', 'rehearse-signed-update.cjs');

/*
 * Failure inventory for the isolated rehearsal path:
 * - a host/stage mismatch, symlinked candidate file, unsigned manifest, bad
 *   archive checksum, invalid native proof, or CLI/archive digest mismatch
 *   must stop before any update fixture is staged;
 * - a mutated archive or signature must be rejected by the real CLI while
 *   both fixture invokers remain byte-identical and the pending package stays;
 * - a valid pending package must recover through each invoker and install the
 *   exact two archive digests before the package is removed;
 * - a Windows sibling delete/share lock must leave the package recoverable
 *   after primary replacement, then retry successfully through the sibling;
 * - unsupported native interruption checks must be named in the artifact,
 *   while path collisions, output symlinks, and fixture cleanup errors fail
 *   closed without overwriting existing user files.
 *
 * The runtime archive/signature cases need a real signed candidate stage and
 * therefore remain exercised by invoking this script against that stage.
 */

function run(args) {
  return spawnSync(process.execPath, [rehearsal, ...args], {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
  });
}

test('help describes the explicit offline candidate-stage contract', () => {
  const result = run(['--help']);

  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /--stage-dir <path>/);
  assert.match(result.stdout, /--binary-dir <path>/);
  assert.match(result.stdout, /--output <path>/);
  assert.match(result.stdout, /offline/i);
  assert.match(result.stdout, /pinned/i);
  assert.match(result.stdout, /file identity/i);
});

test('stage directory is required before any candidate files are inspected', () => {
  const result = run([]);

  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 2, result.stderr);
  assert.match(result.stderr, /--stage-dir is required/);
});

test('unknown signature bypass options are rejected before stage access', () => {
  const result = run(['--stage-dir', '/path/that/must/not-be-read', '--allow-unsigned']);

  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 2, result.stderr);
  assert.match(result.stderr, /Unknown option: --allow-unsigned/);
});

test('focused signed-update rehearsal suite is part of quality:node', () => {
  const packageJson = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));

  assert.match(packageJson.scripts['quality:node'], /signed-update-rehearsal\.test\.cjs/);
});
