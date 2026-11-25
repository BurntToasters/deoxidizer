'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const {
  isPrerelease,
  syncVersion,
  updateCargoLock,
  updateCargoToml,
  updateChangelog,
  updatePackageLock,
  validateChangelogReleaseLinks,
  validateVersion,
} = require('../../scripts/sync-version.cjs');
const { synchronizeBranch } = require('../../scripts/branch-sync.cjs');

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

  const urls = Array.from(
    updated.matchAll(/\]\((https:\/\/github\.com\/BurntToasters\/deoxidizer\/releases\/download\/v[^/]+\/[^)\s]+)\)/g),
    (match) => match[1],
  );
  assert.deepEqual(urls, [
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-windows-x86_64.zip',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-windows-aarch64.zip',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-darwin-x86_64.tar.gz',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-darwin-aarch64.tar.gz',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-linux-x86_64.tar.gz',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-linux-aarch64.tar.gz',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-windows-x86_64-setup.exe',
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.1/deoxidizer-v0.1.1-windows-aarch64-setup.exe',
  ]);
  assert.deepEqual(validateChangelogReleaseLinks(updated, '0.1.1'), []);
});

test('release link validation rejects stale tags, basenames, missing and duplicate URLs', () => {
  const changelog = fs.readFileSync(path.join(root, 'CHANGELOG.md'), 'utf8');
  const urls = Array.from(
    changelog.matchAll(/\]\((https:\/\/github\.com\/BurntToasters\/deoxidizer\/releases\/download\/v[^/]+\/[^)\s]+)\)/g),
    (match) => match[1],
  );
  assert.equal(urls.length, 8);

  const staleTag = changelog.replace(
    'releases/download/v0.1.0/deoxidizer-v0.1.0-windows-x86_64.zip',
    'releases/download/v0.0.9/deoxidizer-v0.1.0-windows-x86_64.zip',
  );
  assert.match(validateChangelogReleaseLinks(staleTag, '0.1.0').join('\n'), /canonical|expected|missing/i);

  const staleBasename = changelog.replace(
    'deoxidizer-v0.1.0-windows-x86_64.zip',
    'deoxidizer-v0.0.9-windows-x86_64.zip',
  );
  assert.match(validateChangelogReleaseLinks(staleBasename, '0.1.0').join('\n'), /canonical|expected|missing/i);

  const missingUrl = changelog.replace(
    'https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-x86_64.zip',
    'https://example.invalid/deoxidizer-v0.1.0-windows-x86_64.zip',
  );
  assert.notDeepEqual(validateChangelogReleaseLinks(missingUrl, '0.1.0'), []);

  const duplicateUrl = changelog.replace(
    '| **Setup:**',
    '| [duplicate](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/deoxidizer-v0.1.0-windows-x86_64.zip) | **Setup:**',
  );
  assert.match(validateChangelogReleaseLinks(duplicateUrl, '0.1.0').join('\n'), /duplicate|exactly once|count/i);
});

test('rejects malformed versions and fixtures', () => {
  for (const bad of ['', 'v1.2.3', '1.0', '1.0.0-beta', '1.0.0-alpha.0-beta', '01.0.0']) {
    assert.throws(() => validateVersion(bad), /Invalid release version/);
  }
  assert.throws(
    () => updateCargoToml('[package]\nname = "deoxidizer"\n', '0.1.1'),
    /exactly once/,
  );
  assert.throws(() => updateCargoToml('no sections here', '0.1.1'), /no \[package\]/);
  assert.throws(() => updateCargoLock('empty lockfile', '0.1.1'), /exactly once/);
  assert.throws(
    () => updatePackageLock(JSON.stringify({ version: '0.1.0' }), '0.1.1'),
    /packages\[""\]/,
  );
  assert.throws(
    () => updateChangelog('# no heading\n', '0.1.0', '0.1.1'),
    /missing current release heading/,
  );
});

test('shared prerelease predicate mirrors the accepted version suffix', () => {
  assert.equal(isPrerelease(validateVersion('0.2.0-beta.1')), true);
  assert.equal(isPrerelease('0.1.0'), false);
  assert.equal(isPrerelease('0.2.0-rc.3'), true);
  assert.equal(isPrerelease('0.2.0'), false);
});

test('no-arg sync propagates package.json version with no edits when consistent', () => {
  const files = ['package.json', 'package-lock.json', 'Cargo.toml', 'Cargo.lock', 'CHANGELOG.md'];
  const before = new Map(files.map((name) => [name, fs.readFileSync(path.join(root, name))]));
  const version = syncVersion(undefined);
  assert.equal(version, JSON.parse(before.get('package.json').toString()).version);
  for (const name of files) {
    assert.deepEqual(fs.readFileSync(path.join(root, name)), before.get(name));
  }
});

function writeVersionFixture(dir, { packageVersion, cargoVersion: cargoVer }) {
  fs.writeFileSync(
    path.join(dir, 'package.json'),
    `${JSON.stringify({ name: 'deoxidizer', version: packageVersion, private: true }, null, 2)}\n`,
  );
  fs.writeFileSync(
    path.join(dir, 'package-lock.json'),
    `${JSON.stringify({ name: 'deoxidizer', version: '0.1.0', lockfileVersion: 3, packages: { '': { name: 'deoxidizer', version: '0.1.0' } } }, null, 2)}\n`,
  );
  fs.writeFileSync(
    path.join(dir, 'Cargo.toml'),
    `[package]\nname = "deoxidizer"\nversion = "${cargoVer}"\nrust-version = "1.98"\n`,
  );
  fs.writeFileSync(
    path.join(dir, 'Cargo.lock'),
    `version = 4\n\n[[package]]\nname = "deoxidizer"\nversion = "0.1.0"\n`,
  );
  const assets = [
    'deoxidizer-v0.1.0-windows-x86_64.zip',
    'deoxidizer-v0.1.0-windows-aarch64.zip',
    'deoxidizer-v0.1.0-darwin-x86_64.tar.gz',
    'deoxidizer-v0.1.0-darwin-aarch64.tar.gz',
    'deoxidizer-v0.1.0-linux-x86_64.tar.gz',
    'deoxidizer-v0.1.0-linux-aarch64.tar.gz',
    'deoxidizer-v0.1.0-windows-x86_64-setup.exe',
    'deoxidizer-v0.1.0-windows-aarch64-setup.exe',
  ];
  const links = assets.map(
    (asset) => `[${asset}](https://github.com/BurntToasters/deoxidizer/releases/download/v0.1.0/${asset})`,
  );
  fs.writeFileSync(
    path.join(dir, 'CHANGELOG.md'),
    `# Changelog\n\n# ⬇️ Downloads\n\n${links.join(' | ')}\n\n> [!IMPORTANT]\n> note\n\n## Changes in \`v0.1.0:\`\n\n- stuff\n`,
  );
}

test('no-arg sync propagates a hand-bumped package.json version everywhere', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'sync-version-pkg-bump-'));
  try {
    writeVersionFixture(dir, { packageVersion: '0.1.1', cargoVersion: '0.1.0' });
    assert.equal(syncVersion(undefined, dir), '0.1.1');
    assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8')).version, '0.1.1');
    assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'package-lock.json'), 'utf8')).version, '0.1.1');
    assert.match(fs.readFileSync(path.join(dir, 'Cargo.toml'), 'utf8'), /^version = "0\.1\.1"$/m);
    assert.match(fs.readFileSync(path.join(dir, 'Cargo.lock'), 'utf8'), /name = "deoxidizer"\nversion = "0\.1\.1"/);
    const changelog = fs.readFileSync(path.join(dir, 'CHANGELOG.md'), 'utf8');
    assert.match(changelog, /## Changes in `v0\.1\.1:`/);
    assert.match(changelog, /releases\/download\/v0\.1\.1\//);
    assert.doesNotMatch(changelog, /releases\/download\/v0\.1\.0\//);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('no-arg sync fails closed when only Cargo.toml was bumped', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'sync-version-cargo-drift-'));
  try {
    writeVersionFixture(dir, { packageVersion: '0.1.0', cargoVersion: '0.1.1' });
    const before = fs.readFileSync(path.join(dir, 'Cargo.toml'), 'utf8');
    assert.throws(() => syncVersion(undefined, dir), /missing current release heading/);
    assert.equal(fs.readFileSync(path.join(dir, 'Cargo.toml'), 'utf8'), before);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('branch sync checks the selected remote branch before destructive cleanup', () => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'branch-sync-preflight-'));
  const bareRemote = path.join(fixture, 'origin.git');
  const worktree = path.join(fixture, 'worktree');
  const git = (cwd, args) => {
    const result = spawnSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe' });
    if (result.error) throw result.error;
    assert.equal(result.status, 0, result.stderr);
    return result;
  };

  try {
    fs.mkdirSync(worktree);
    git(fixture, ['init', '--bare', bareRemote]);
    git(worktree, ['init']);
    git(worktree, ['checkout', '-b', 'main']);
    git(worktree, ['config', 'user.name', 'Deoxidizer Fixture']);
    git(worktree, ['config', 'user.email', 'fixture@example.invalid']);
    fs.writeFileSync(path.join(worktree, 'tracked.txt'), 'fixture\n');
    git(worktree, ['add', 'tracked.txt']);
    git(worktree, ['commit', '-m', 'fixture']);
    git(worktree, ['remote', 'add', 'origin', bareRemote]);
    git(worktree, ['push', '--set-upstream', 'origin', 'main']);
    fs.writeFileSync(path.join(worktree, 'untracked-sentinel.txt'), 'keep me\n');

    const executed = [];
    const execute = (command, args, options = {}) => {
      executed.push([command, ...args]);
      const result = spawnSync(command, args, {
        cwd: options.cwd,
        env: options.env,
        encoding: 'utf8',
        stdio: 'pipe',
      });
      if (result.error) throw result.error;
      if (result.status !== 0) throw new Error(`${command} failed: ${result.stderr}`);
      return result;
    };

    assert.throws(
      () => synchronizeBranch('beta', {
        cwd: worktree,
        env: { DEOX_RELEASE_CONFIRM: 'YES' },
        execute,
      }),
      /origin\/beta.*(?:missing|does not exist|not found)/i,
    );
    assert.deepEqual(executed, [['git', 'fetch', 'origin']]);
    assert.equal(fs.readFileSync(path.join(worktree, 'untracked-sentinel.txt'), 'utf8'), 'keep me\n');
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});
