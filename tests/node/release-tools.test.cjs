'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const {
  normalizeArch,
  normalizeOs,
  TARGETS,
  buildEnvironment,
} = require('../../scripts/release.cjs');
const { githubCliEnvironment } = require('../../scripts/github-cli.cjs');
const {
  archiveEntries,
  verifyZipIntegrity,
  parseChecksumManifest,
  verifyChecksum,
  validateRemoteAssetNames,
  validateRemoteManifestEntries,
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

function storedZip(names) {
  const localParts = [];
  const centralParts = [];
  let localOffset = 0;

  for (const name of names) {
    const filename = Buffer.from(name);
    const local = Buffer.alloc(30 + filename.length);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(filename.length, 26);
    filename.copy(local, 30);
    localParts.push(local);

    const central = Buffer.alloc(46 + filename.length);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(filename.length, 28);
    central.writeUInt32LE(localOffset, 42);
    filename.copy(central, 46);
    centralParts.push(central);
    localOffset += local.length;
  }

  const localData = Buffer.concat(localParts);
  const centralData = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(names.length, 8);
  end.writeUInt16LE(names.length, 10);
  end.writeUInt32LE(centralData.length, 12);
  end.writeUInt32LE(localData.length, 16);
  return Buffer.concat([localData, centralData, end]);
}

test('normalizes release aliases to supported target triples', () => {
  assert.equal(normalizeOs('macos'), 'darwin');
  assert.equal(normalizeOs('win'), 'windows');
  assert.equal(normalizeArch('amd64', 'linux'), 'x86_64');
  assert.equal(normalizeArch('arm64', 'linux'), 'aarch64');
  assert.equal(TARGETS.linux.aarch64, 'aarch64-unknown-linux-gnu');
});

test('rejects host architecture for a different operating system', () => {
  const hostOs =
    process.platform === 'darwin'
      ? 'darwin'
      : process.platform === 'win32'
        ? 'windows'
        : 'linux';
  const differentOs = hostOs === 'linux' ? 'darwin' : 'linux';
  assert.throws(() => normalizeArch('host', differentOs), /host architecture/);
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

test('inspects ZIP archives without an external unzip command', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'release.zip');
  const names = ['deoxidizer.exe', 'deox.exe', 'LICENSE'];
  fs.writeFileSync(filePath, storedZip(names));
  assert.deepEqual(archiveEntries(filePath), new Set(names));
  assert.doesNotThrow(() => verifyZipIntegrity(filePath));
  fs.rmSync(directory, { recursive: true, force: true });
});

test('ZIP integrity checker rejects CRC drift', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'release.zip');
  const bytes = storedZip(['deoxidizer', 'deox', 'LICENSE']);
  // First central-directory entry starts after three empty local headers.
  const centralOffset = bytes.length - 22 - (46 + 'deoxidizer'.length) - (46 + 'deox'.length) - (46 + 'LICENSE'.length);
  bytes.writeUInt32LE(1, centralOffset + 16);
  fs.writeFileSync(filePath, bytes);
  assert.throws(() => verifyZipIntegrity(filePath), /CRC/);
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

test('release build and upload environment excludes signing credentials', () => {
  const environment = buildEnvironment({
    GPG_PASSPHRASE: 'secret',
    AZURE_CLIENT_SECRET: 'secret',
    APPLE_PASSWORD: 'secret',
    PATH: '/bin',
  });
  assert.equal(environment.GPG_PASSPHRASE, undefined);
  assert.equal(environment.AZURE_CLIENT_SECRET, undefined);
  assert.equal(environment.APPLE_PASSWORD, undefined);
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

test('draft validator rejects commit drift and unexpected assets', () => {
  const expected = [...expectedReleaseAssets()];
  const assets = expected.map((name) => ({ name, size: 1 }));
  assets.push({ name: 'unexpected.bin', size: 1 });
  const errors = validateDraft(
    { draft: true, prerelease, tag_name: tag, target_commitish: 'b'.repeat(40) },
    assets,
    'a'.repeat(40),
  );
  assert.match(errors.join('\n'), /target commit/);
  assert.match(errors.join('\n'), /unexpected asset/);
});

test('remote validator permits earlier target assets but rejects unknown names', () => {
  const expected = [...expectedReleaseAssets()];
  assert.deepEqual(
    validateRemoteAssetNames(expected.slice(0, 4).map((name) => ({ name }))),
    [],
  );
  assert.match(
    validateRemoteAssetNames([{ name: expected[0] }, { name: 'unexpected.bin' }]).join('\n'),
    /unexpected asset/,
  );
  assert.match(
    validateRemoteAssetNames([{ name: expected[0] }, { name: expected[0] }]).join('\n'),
    /duplicate asset/,
  );
});

test('remote manifest validator binds signed entries to remote asset digests', () => {
  const archive = `deoxidizer-v${tag.slice(1)}-linux-x86_64.tar.gz`;
  const digest = 'a'.repeat(64);
  const assets = new Map([[archive, { name: archive, digest: `sha256:${digest}` }]]);
  assert.deepEqual(
    validateRemoteManifestEntries(
      'SHA256SUMS-linux-x86_64.txt',
      `${digest}  ${archive}\n`,
      assets,
    ),
    [],
  );
  assert.match(
    validateRemoteManifestEntries(
      'SHA256SUMS-linux-x86_64.txt',
      `${'b'.repeat(64)}  ${archive}\n`,
      assets,
    ).join('\n'),
    /digest mismatch/,
  );
});

test('GitHub draft notes come from BCLS changelog', () => {
  assert.match(releaseNotes(), /## Changes in `v0\.1\.0:`/);
  assert.match(releaseNotes(), /BCLS standard/);
});
