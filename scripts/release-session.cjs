'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');
const qualityPath = path.join(releaseDir, '.release-quality.json');
const sessionPath = path.join(releaseDir, '.build-session.json');
const MAX_AGE_MS = 24 * 60 * 60 * 1000;

function command(name, args) {
  return execFileSync(name, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
}

function assertCleanCheckout() {
  if (command('git', ['status', '--porcelain', '--untracked-files=all'])) {
    throw new Error('Release requires a clean Git checkout; commit or stash changes first');
  }
}

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function cargoVersion() {
  const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  const match = manifest.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('Cargo.toml has no package version');
  return match[1];
}

function currentIdentity(target = process.env.DEOX_RELEASE_TARGET || 'host') {
  assertCleanCheckout();
  return {
    version: cargoVersion(),
    commit: command('git', ['rev-parse', 'HEAD']),
    target,
    platform: process.platform,
    arch: process.arch,
    node: process.version,
    rustc: command('rustc', ['--version']),
    cargoLockSha256: sha256File(path.join(root, 'Cargo.lock')),
    packageLockSha256: fs.existsSync(path.join(root, 'package-lock.json'))
      ? sha256File(path.join(root, 'package-lock.json'))
      : null,
  };
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

function recordQualityGate(target) {
  const proof = {
    ...currentIdentity(target),
    completedAt: Date.now(),
  };
  writeJson(qualityPath, proof);
  return proof;
}

function readJson(filePath, label) {
  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch (error) {
    throw new Error(
      `${label} is missing or invalid: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

function validateIdentity(record, expected, label) {
  for (const [key, value] of Object.entries(expected)) {
    if (record[key] !== value) {
      throw new Error(`${label} ${key} does not match current checkout or target`);
    }
  }
}

function validateFresh(record, label, now = Date.now()) {
  if (!Number.isFinite(record?.completedAt ?? record?.startedAt)) {
    throw new Error(`${label} has no valid timestamp`);
  }
  const timestamp = record.completedAt ?? record.startedAt;
  const age = now - timestamp;
  if (age < 0 || age > MAX_AGE_MS) {
    throw new Error(`${label} is expired; run release:prepare again`);
  }
}

function startSession(target) {
  const expected = currentIdentity(target);
  const proof = readJson(qualityPath, 'Release quality gate');
  validateFresh(proof, 'Release quality gate');
  validateIdentity(proof, expected, 'Release quality gate');
  const session = {
    ...expected,
    qualityCompletedAt: proof.completedAt,
    startedAt: Date.now(),
  };
  writeJson(sessionPath, session);
  return session;
}

function verifySession(target) {
  const expected = currentIdentity(target);
  const session = readJson(sessionPath, 'Release build session');
  validateFresh(session, 'Release build session');
  validateIdentity(session, expected, 'Release build session');
  if (!Number.isFinite(session.qualityCompletedAt)) {
    throw new Error('Release build session has no quality-gate proof');
  }
  return session;
}

function main() {
  const action = process.argv[2];
  const target = process.argv[3] || process.env.DEOX_RELEASE_TARGET || 'host';
  if (action === 'quality') {
    const proof = recordQualityGate(target);
    console.log(`release quality gate recorded (${proof.version}, ${proof.target})`);
    return;
  }
  if (action === 'start') {
    const session = startSession(target);
    console.log(`release session started (${session.version}, ${session.target})`);
    return;
  }
  if (action === 'verify') {
    const session = verifySession(target);
    console.log(`release session verified (${session.version}, ${session.target})`);
    return;
  }
  throw new Error('usage: node scripts/release-session.cjs <quality|start|verify> [target]');
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = {
  MAX_AGE_MS,
  currentIdentity,
  validateFresh,
  validateIdentity,
};
