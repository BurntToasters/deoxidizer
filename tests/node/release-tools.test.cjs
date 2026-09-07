'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const { normalizeArch, normalizeOs, TARGETS } = require('../../scripts/release.cjs');
const { githubCliEnvironment } = require('../../scripts/github-cli.cjs');
const {
  parseChecksumManifest,
  verifyChecksum,
} = require('../../scripts/verify-release.cjs');
const { validateFresh, validateIdentity } = require('../../scripts/release-session.cjs');
const { requireConfirmation } = require('../../scripts/branch-sync.cjs');
const {
  expectedReleaseAssets,
  validateDraft,
  tag,
  prerelease,
} = require('../../scripts/verify-release-draft.cjs');
const { releaseNotes } = require('../../scripts/ensure-draft-release.cjs');

test('normalizes release aliases to supported target triples', () => {
  assert.equal(normalizeOs('macos'), 'darwin');
  assert.equal(normalizeOs('win'), 'windows');
  assert.equal(normalizeArch('amd64', 'linux'), 'x86_64');
  assert.equal(normalizeArch('arm64', 'linux'), 'aarch64');
  assert.equal(TARGETS.linux.aarch64, 'aarch64-unknown-linux-gnu');
});

test('rejects host architecture for a different operating system', () => {
  assert.throws(() => normalizeArch('host', 'linux'), /host architecture/);
});

test('parses checksum manifests and rejects duplicates', () => {
  const manifest = parseChecksumManifest(
    'a'.repeat(64) + '  deoxidizer-v0.1.0-linux-x86_64.tar.gz\n',
  );
  assert.equal(manifest.size, 1);
  assert.throws(
    () =>
      parseChecksumManifest(
        `${'a'.repeat(64)}  one\n${'b'.repeat(64)}  one\n`,
      ),
    /Duplicate checksum/,
  );
});

test('verifies local checksum bytes', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'artifact');
  const bytes = Buffer.from('release artifact');
  fs.writeFileSync(filePath, bytes);
  const digest = crypto.createHash('sha256').update(bytes).digest('hex');
  assert.equal(verifyChecksum(filePath, digest), true);
  assert.equal(verifyChecksum(filePath, '0'.repeat(64)), false);
  fs.rmSync(directory, { recursive: true, force: true });
});

test('scrubs GitHub token environment variables', () => {
  const environment = githubCliEnvironment({
    GH_TOKEN: 'secret',
    GITHUB_TOKEN: 'secret',
    PATH: '/bin',
  });
  assert.equal(environment.GH_TOKEN, undefined);
  assert.equal(environment.GITHUB_TOKEN, undefined);
  assert.equal(environment.PATH, '/bin');
});

test('release identity validation rejects drift and expiry', () => {
  assert.doesNotThrow(() =>
    validateIdentity({ version: '1', commit: 'abc' }, { version: '1', commit: 'abc' }, 'proof'),
  );
  assert.throws(
    () => validateIdentity({ version: '1' }, { version: '2' }, 'proof'),
    /version/,
  );
  assert.throws(
    () => validateFresh({ completedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );
});

test('destructive branch sync requires explicit confirmation', () => {
  const previous = process.env.DEOX_RELEASE_CONFIRM;
  delete process.env.DEOX_RELEASE_CONFIRM;
  assert.throws(() => requireConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
  if (previous === undefined) delete process.env.DEOX_RELEASE_CONFIRM;
  else process.env.DEOX_RELEASE_CONFIRM = previous;
});

test('draft validator requires every supported target manifest and archive', () => {
  const expected = [...expectedReleaseAssets()];
  const assets = expected.map((name) => ({ name, size: 1 }));
  assert.deepEqual(validateDraft({ draft: true, prerelease, tag_name: tag }, assets), []);
  assert.match(
    validateDraft({ draft: true, prerelease, tag_name: tag }, assets.slice(1)).join('\n'),
    /missing asset/,
  );
});

test('GitHub draft notes come from BCLS changelog', () => {
  assert.match(releaseNotes(), /## Changes in `v0\.1\.0:`/);
  assert.match(releaseNotes(), /BCLS standard/);
});
