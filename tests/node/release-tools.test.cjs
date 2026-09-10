'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const { execFileSync, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const packageManifest = require('../../package.json');

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
const { requireConfirmation, parseArgs } = require('../../scripts/branch-sync.cjs');
const { requireConfirmation: requireViConfirmation } = require('../../scripts/vi.cjs');
const {
  requireConfirmation: requirePruneConfirmation,
} = require('../../scripts/git-prune.cjs');
const { expectedFingerprint } = require('../../scripts/check-release-key.cjs');
const { isPrerelease } = require('../../scripts/sync-version.cjs');
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

test('release scripts match the manual VM draft ownership protocol', () => {
  assert.match(packageManifest.scripts['release:windows'], /--upload/);
  assert.doesNotMatch(packageManifest.scripts['release:windows'], /--wait/);
  for (const scriptName of [
    'release:linux',
    'release:linux:x64',
    'release:linux:arm64',
    'release:macos',
    'release:macos:x64',
    'release:macos:arm64',
  ]) {
    assert.match(packageManifest.scripts[scriptName], /--upload/);
    assert.match(packageManifest.scripts[scriptName], /--wait/);
  }
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
  assert.equal(tag, `v${packageManifest.version}`);
  const manifest = parseChecksumManifest(
    `${'a'.repeat(64)}  deoxidizer-v${packageManifest.version}-linux-x86_64.tar.gz\n`,
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

test('branch sync --force-always bypasses confirmation gate', () => {
  assert.deepEqual(parseArgs(['main', '--force-always']), {
    branch: 'main',
    forceAlways: true,
  });
  assert.deepEqual(parseArgs(['main']), { branch: 'main', forceAlways: false });
  assert.deepEqual(parseArgs(['--force-always', 'beta']), {
    branch: 'beta',
    forceAlways: true,
  });
  assert.throws(() => parseArgs(['main', '--bogus']), /unknown flag/);
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
  assert.equal(tag, `v${packageManifest.version}`);
  const escaped = packageManifest.version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  assert.match(releaseNotes(), new RegExp(`## Changes in \`v${escaped}:\``));
  assert.match(releaseNotes(), /BCLS standard/);
});

// Stage a script under a temp repo root so its __dirname-based `root`
// resolves to a synthetic fixture instead of the real repository. The
// staged copies are never written back to scripts/.
function stageScript(scriptName, files) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-script-test-'));
  const scriptDir = path.join(directory, 'scripts');
  fs.mkdirSync(scriptDir, { recursive: true });
  fs.copyFileSync(
    path.join(__dirname, '../../scripts', scriptName),
    path.join(scriptDir, scriptName),
  );
  for (const [relative, content] of Object.entries(files)) {
    const target = path.join(directory, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
    if (relative === 'bin/gpg') fs.chmodSync(target, 0o755);
  }
  return directory;
}

function runStagedScript(directory, scriptName, extraEnv) {
  try {
    return spawnSync(process.execPath, [path.join(directory, 'scripts', scriptName)], {
      encoding: 'utf8',
      env: extraEnv === undefined ? process.env : { ...process.env, ...extraEnv },
    });
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test('check-license rejects a fixture with the wrong declared license', () => {
  const licenseHeader = `${' '.repeat(20)}GNU GENERAL PUBLIC LICENSE\n`;
  const good = {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "0.1.0"\nlicense = "GPL-3.0-or-later"\n',
    'package.json': JSON.stringify({ license: 'GPL-3.0-or-later' }),
    'README.md': 'licensed under GPL-3.0-or-later\n',
    'AGENTS.md': 'licensed under GPL-3.0-or-later\n',
    LICENSE: `${licenseHeader}Version 3 text\n`,
  };
  const goodDir = stageScript('check-license.cjs', good);
  assert.equal(runStagedScript(goodDir, 'check-license.cjs').status, 0);

  const badDir = stageScript('check-license.cjs', {
    ...good,
    'Cargo.toml': good['Cargo.toml'].replace('GPL-3.0-or-later', 'MIT'),
  });
  const bad = runStagedScript(badDir, 'check-license.cjs');
  assert.notEqual(bad.status, 0);
  assert.match(bad.stderr, /Cargo\.toml must declare/);
});

test('check-version rejects mismatched fixture versions', () => {
  const directory = stageScript('check-version.cjs', {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "9.9.9"\n',
    'package.json': JSON.stringify({ version: '0.0.0' }),
  });
  const result = runStagedScript(directory, 'check-version.cjs');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not match/);
});

test('check-changelog rejects a fixture missing BCLS markers', () => {
  const directory = stageScript('check-changelog.cjs', {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "0.1.0"\n',
    'CHANGELOG.md': '# empty changelog\n',
  });
  const result = runStagedScript(directory, 'check-changelog.cjs');
  assert.notEqual(result.status, 0);
  assert.match(result.stdout + result.stderr, /missing changelog marker/);
});

// Fixture pins are read from repo files at runtime so toolchain bumps do not
// cause fixture drift. The synthetic scripts/release.cjs snippet mirrors the
// real invocation line asserted below.
function repoToolchainPins() {
  const repoRoot = path.resolve(__dirname, '../..');
  const toolchain = fs
    .readFileSync(path.join(repoRoot, 'rust-toolchain.toml'), 'utf8')
    .match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  const nodePin = fs.readFileSync(path.join(repoRoot, '.node-version'), 'utf8').trim();
  const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, 'package.json'), 'utf8'));
  const npmPin = /npm@([\d.]+)/.exec(packageJson.packageManager ?? '')?.[1];
  const npmEngines = packageJson.engines?.npm;
  const rustVersion = fs
    .readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8')
    .match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];
  assert.ok(toolchain, 'repo rust-toolchain.toml must pin a channel');
  assert.ok(nodePin, 'repo .node-version must pin a version');
  assert.ok(npmPin, 'repo package.json must pin packageManager npm');
  assert.ok(npmEngines, 'repo package.json must declare engines.npm');
  assert.ok(rustVersion, 'repo Cargo.toml must declare rust-version');
  return { toolchain, nodePin, npmPin, npmEngines, rustVersion };
}

function consistentToolchainFixture() {
  const { toolchain, nodePin, npmPin, npmEngines, rustVersion } = repoToolchainPins();
  const sha = 'a'.repeat(40);
  const workflow = (extra = '') =>
    `jobs:\n  build:\n    steps:\n      - uses: dtolnay/rust-toolchain@${sha}\n        with:\n          toolchain: ${toolchain}\n      - uses: actions/setup-node@v4\n        with:\n          node-version: ${nodePin}\n      - run: npm install --global npm@${npmPin}\n${extra}`;
  return {
    'rust-toolchain.toml': `[toolchain]\nchannel = "${toolchain}"\n`,
    'Cargo.toml': `[package]\nname = "deoxidizer"\nversion = "0.1.0"\nrust-version = "${rustVersion}"\n`,
    '.node-version': `${nodePin}\n`,
    'package.json': JSON.stringify({
      packageManager: `npm@${npmPin}`,
      engines: { npm: npmEngines },
    }),
    '.github/workflows/ci.yml': workflow(),
    '.github/workflows/release.yml': workflow(),
    'scripts/release.cjs':
      `run('rustup', ['target', 'add', '--toolchain', '${toolchain}', target], buildEnv);\n`,
  };
}

test('check-toolchain main accepts a consistent fixture, rejects pin drift', () => {
  const { toolchain } = repoToolchainPins();
  // The synthetic snippet must mirror the real scripts/release.cjs invocation line.
  const realRelease = fs.readFileSync(
    path.join(__dirname, '../../scripts/release.cjs'),
    'utf8',
  );
  const escaped = toolchain.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  assert.match(realRelease, new RegExp(`--toolchain',\\s*'${escaped}'`));
  const fixture = consistentToolchainFixture();
  assert.ok(fixture['scripts/release.cjs'].includes(`'${toolchain}'`));
  const goodDir = stageScript('check-toolchain.cjs', fixture);
  const stagedGood = require(path.join(goodDir, 'scripts/check-toolchain.cjs'));
  assert.doesNotThrow(() => stagedGood.main());
  fs.rmSync(goodDir, { recursive: true, force: true });

  const badFiles = consistentToolchainFixture();
  badFiles['.github/workflows/ci.yml'] = badFiles['.github/workflows/ci.yml'].replace(
    `toolchain: ${toolchain}`,
    'toolchain: 0.0.0',
  );
  const badDir = stageScript('check-toolchain.cjs', badFiles);
  try {
    const stagedBad = require(path.join(badDir, 'scripts/check-toolchain.cjs'));
    assert.throws(() => stagedBad.main(), /pinned Rust toolchain/);
  } finally {
    fs.rmSync(badDir, { recursive: true, force: true });
  }
});

test('check-release-key main rejects a fixture missing the pinned fingerprint', () => {
  const gpgStub = `#!/bin/sh\necho "fpr:::::::::${expectedFingerprint}:"\n`;
  const goodFiles = {
    'release-signing-key.asc': 'fixture key\n',
    'install.sh': `KEY="${expectedFingerprint}"\n`,
    'install.ps1': `KEY="${expectedFingerprint}"\n`,
    'bin/gpg': gpgStub,
  };
  const goodDir = stageScript('check-release-key.cjs', goodFiles);
  const stagedGood = require(path.join(goodDir, 'scripts/check-release-key.cjs'));
  const previousPath = process.env.PATH;
  process.env.PATH = `${path.join(goodDir, 'bin')}${path.delimiter}${previousPath}`;
  try {
    assert.doesNotThrow(() => stagedGood.main());
  } finally {
    process.env.PATH = previousPath;
    fs.rmSync(goodDir, { recursive: true, force: true });
  }

  const badDir = stageScript('check-release-key.cjs', {
    ...goodFiles,
    'install.sh': 'echo no key pinned here\n',
  });
  try {
    const stagedBad = require(path.join(badDir, 'scripts/check-release-key.cjs'));
    process.env.PATH = `${path.join(badDir, 'bin')}${path.delimiter}${previousPath}`;
    try {
      assert.throws(() => stagedBad.main(), /does not pin/);
    } finally {
      process.env.PATH = previousPath;
    }
  } finally {
    fs.rmSync(badDir, { recursive: true, force: true });
  }
});

test('vi and git-prune refuse without explicit confirmation', () => {
  const previous = process.env.DEOX_RELEASE_CONFIRM;
  delete process.env.DEOX_RELEASE_CONFIRM;
  try {
    assert.throws(() => requireViConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
    assert.throws(() => requirePruneConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
  } finally {
    if (previous === undefined) delete process.env.DEOX_RELEASE_CONFIRM;
    else process.env.DEOX_RELEASE_CONFIRM = previous;
  }
});

test('publish-release requires --yes before touching the network', () => {
  // No --yes and no confirm env: must fail in the local gate, no gh calls.
  const result = spawnSync(
    process.execPath,
    [path.join(__dirname, '../../scripts/publish-release.cjs')],
    { encoding: 'utf8' },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /--yes/);
});

test('shared prerelease predicate matches only suffixed beta-style versions', () => {
  assert.equal(isPrerelease('0.2.0-beta.1'), true);
  assert.equal(isPrerelease('0.2.0-rc.0'), true);
  assert.equal(isPrerelease('0.2.0-alpha.12'), true);
  assert.equal(isPrerelease('0.1.0'), false);
  assert.equal(isPrerelease('0.2.0-beta'), false);
  assert.equal(isPrerelease('0.2.0-BETA.1'), false);
  const source = fs.readFileSync(
    path.join(__dirname, '../../scripts/publish-release.cjs'),
    'utf8',
  );
  assert.match(source, /isPrerelease\(version\)/);
});

test('release env scoping keeps only OS signing keys', () => {
  // signingEnvironment/gpgEnvironment are intentionally unexported, so load a
  // staged copy with the same appended export instead of editing scripts/.
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-env-test-'));
  const scriptDir = path.join(directory, 'scripts');
  fs.mkdirSync(scriptDir, { recursive: true });
  for (const scriptName of ['release.cjs', 'github-cli.cjs', 'sync-version.cjs']) {
    fs.copyFileSync(
      path.join(__dirname, '../../scripts', scriptName),
      path.join(scriptDir, scriptName),
    );
  }
  const stagedPath = path.join(scriptDir, 'release.cjs');
  fs.appendFileSync(
    stagedPath,
    '\nmodule.exports.signingEnvironment = signingEnvironment;\nmodule.exports.gpgEnvironment = gpgEnvironment;\n',
  );
  let staged;
  try {
    staged = require(stagedPath);
  } catch (error) {
    fs.rmSync(directory, { recursive: true, force: true });
    throw error;
  }
  try {
    const env = {
      PATH: '/bin',
      GPG_KEY_ID: 'key',
      GPG_PASSPHRASE: 'pass',
      AZURE_CLIENT_SECRET: 'secret',
      APPLE_PASSWORD: 'apple',
      APPLE_ID: 'id',
    };
    const darwin = staged.signingEnvironment(env, 'darwin');
    assert.equal(darwin.APPLE_ID, 'id');
    assert.equal(darwin.AZURE_CLIENT_SECRET, undefined);
    assert.equal(darwin.GPG_PASSPHRASE, undefined);
    assert.equal(darwin.PATH, '/bin');

    const windows = staged.signingEnvironment(env, 'windows');
    assert.equal(windows.AZURE_CLIENT_SECRET, 'secret');
    assert.equal(windows.APPLE_PASSWORD, undefined);
    assert.equal(windows.GPG_KEY_ID, undefined);

    const linux = staged.signingEnvironment(env, 'linux');
    assert.equal(linux.AZURE_CLIENT_SECRET, undefined);
    assert.equal(linux.APPLE_ID, undefined);
    assert.equal(linux.GPG_PASSPHRASE, undefined);

    const gpg = staged.gpgEnvironment(env);
    assert.equal(gpg.GPG_KEY_ID, 'key');
    assert.equal(gpg.GPG_PASSPHRASE, 'pass');
    assert.equal(gpg.AZURE_CLIENT_SECRET, undefined);
    assert.equal(gpg.APPLE_PASSWORD, undefined);
    assert.equal(gpg.PATH, '/bin');
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('release-session rejects expired proofs and dirty checkouts', () => {
  assert.throws(
    () => validateFresh({ completedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );
  assert.throws(
    () => validateFresh({ startedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );

  // assertCleanCheckout is intentionally unexported; exercise it through a
  // staged copy whose root-relative git/reads point at a temp git repo.
  if (spawnSync('git', ['--version'], { stdio: 'ignore' }).status !== 0) return;
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-git-test-'));
  try {
    const scriptDir = path.join(directory, 'scripts');
    fs.mkdirSync(scriptDir, { recursive: true });
    for (const scriptName of ['release-session.cjs', 'sync-version.cjs']) {
      fs.copyFileSync(
        path.join(__dirname, '../../scripts', scriptName),
        path.join(scriptDir, scriptName),
      );
    }
    const git = (args) =>
      execFileSync('git', ['-c', 'init.defaultBranch=main', ...args], {
        cwd: directory,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    fs.writeFileSync(
      path.join(directory, 'Cargo.toml'),
      '[package]\nname = "deoxidizer"\nversion = "0.1.0"\n',
    );
    fs.writeFileSync(path.join(directory, 'Cargo.lock'), 'fixture\n');
    git(['init']);
    const commitEnv = ['-c', 'user.email=test@example.com', '-c', 'user.name=test'];
    git([...commitEnv, 'add', '-A']);
    git([...commitEnv, '-c', 'commit.gpgsign=false', 'commit', '-m', 'init']);
    assert.equal(
      execFileSync('git', ['status', '--porcelain', '--untracked-files=all'], {
        cwd: directory,
        encoding: 'utf8',
      }).trim(),
      '',
    );

    fs.appendFileSync(path.join(directory, 'Cargo.lock'), 'dirty\n');
    const staged = require(path.join(scriptDir, 'release-session.cjs'));
    assert.throws(() => staged.currentIdentity(), /clean Git checkout/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
