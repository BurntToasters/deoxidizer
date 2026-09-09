'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { uploadReleaseAsset } = require('./github-cli.cjs');
const { cargoVersion } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');
const SIGNING_ENV_KEYS = [
  'GPG_KEY_ID',
  'GPG_PASSPHRASE',
  'AZURE_CLIENT_ID',
  'AZURE_TENANT_ID',
  'AZURE_SUBSCRIPTION_ID',
  'AZURE_CLIENT_SECRET',
  'AZURE_ARTIFACT_SIGNING_ENDPOINT',
  'AZURE_ARTIFACT_SIGNING_ACCOUNT',
  'AZURE_ARTIFACT_SIGNING_PROFILE',
  'AZURE_ARTIFACT_SIGNING_PUBLISHER',
  'AZURE_ARTIFACT_SIGNING_PUBLISHER_DN',
  'APPLE_SIGNING_IDENTITY',
  'APPLE_ID',
  'APPLE_PASSWORD',
  'APPLE_TEAM_ID',
  'APPLE_KEYCHAIN_PROFILE',
  // Bypass flags are signing-adjacent: strip from build/quality env so a
  // stray SKIP/ALLOW in the caller cannot silently weaken a signed build.
  'SKIP_WIN_CODESIGN',
  'DEOX_ALLOW_UNSIGNED_RELEASE',
];

const TARGETS = {
  linux: {
    x86_64: 'x86_64-unknown-linux-gnu',
    aarch64: 'aarch64-unknown-linux-gnu',
  },
  darwin: {
    x86_64: 'x86_64-apple-darwin',
    aarch64: 'aarch64-apple-darwin',
  },
  windows: {
    x86_64: 'x86_64-pc-windows-msvc',
    aarch64: 'aarch64-pc-windows-msvc',
  },
};

function packageVersion() {
  return cargoVersion(fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'));
}

function hostArch() {
  if (process.arch === 'arm64') return 'aarch64';
  if (process.arch === 'x64') return 'x86_64';
  throw new Error(`Unsupported host architecture: ${process.arch}`);
}

function hostOs() {
  if (process.platform === 'darwin') return 'darwin';
  if (process.platform === 'linux') return 'linux';
  if (process.platform === 'win32') return 'windows';
  throw new Error(`Unsupported host operating system: ${process.platform}`);
}

function normalizeOs(value) {
  const normalized = value.toLowerCase();
  if (normalized === 'mac' || normalized === 'macos' || normalized === 'darwin') return 'darwin';
  if (normalized === 'win' || normalized === 'windows') return 'windows';
  if (normalized === 'linux') return 'linux';
  throw new Error(`Unsupported release OS: ${value}`);
}

function normalizeArch(value, os) {
  if (value === 'host') {
    if (normalizeOs(os) !== hostOs()) {
      throw new Error(`host architecture is only valid for ${hostOs()} builds`);
    }
    return hostArch();
  }
  const normalized = value.toLowerCase();
  if (['x64', 'amd64', 'x86_64'].includes(normalized)) return 'x86_64';
  if (['arm64', 'aarch64'].includes(normalized)) return 'aarch64';
  throw new Error(`Unsupported release architecture: ${value}`);
}

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, {
    cwd: root,
    env,
    encoding: 'utf8',
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed with status ${result.status}`);
  }
}

function buildEnvironment(env) {
  const sanitized = { ...env };
  for (const key of SIGNING_ENV_KEYS) delete sanitized[key];
  // Never leak GitHub tokens into build/quality/sign child processes; gh
  // uses its credential store instead (see github-cli.cjs).
  delete sanitized.GH_TOKEN;
  delete sanitized.GITHUB_TOKEN;
  return sanitized;
}

function signingEnvironment(env, os) {
  const scoped = { ...env };
  delete scoped.GH_TOKEN;
  delete scoped.GITHUB_TOKEN;
  const keep = new Set();
  if (os === 'darwin') {
    for (const key of ['APPLE_SIGNING_IDENTITY', 'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_KEYCHAIN_PROFILE']) {
      keep.add(key);
    }
  }
  if (os === 'windows') {
    for (const key of [
      'AZURE_CLIENT_ID',
      'AZURE_TENANT_ID',
      'AZURE_SUBSCRIPTION_ID',
      'AZURE_CLIENT_SECRET',
      'AZURE_ARTIFACT_SIGNING_ENDPOINT',
      'AZURE_ARTIFACT_SIGNING_ACCOUNT',
      'AZURE_ARTIFACT_SIGNING_PROFILE',
      'AZURE_ARTIFACT_SIGNING_PUBLISHER',
      'AZURE_ARTIFACT_SIGNING_PUBLISHER_DN',
      'SKIP_WIN_CODESIGN',
    ]) {
      keep.add(key);
    }
  }
  for (const key of SIGNING_ENV_KEYS) {
    if (!keep.has(key)) delete scoped[key];
  }
  return scoped;
}

function gpgEnvironment(env) {
  const scoped = buildEnvironment(env);
  for (const key of ['GPG_KEY_ID', 'GPG_PASSPHRASE']) {
    if (env[key] !== undefined) scoped[key] = env[key];
  }
  return scoped;
}

function releaseFiles() {
  return fs
    .readdirSync(releaseDir)
    .filter((name) => !name.startsWith('.') && fs.statSync(path.join(releaseDir, name)).isFile())
    .map((name) => path.join(releaseDir, name));
}

function upload(tag, files, environment) {
  for (const filePath of files) {
    uploadReleaseAsset(tag, filePath, { clobber: true, environment });
    console.log(`uploaded ${path.basename(filePath)}`);
  }
}

function uploadExisting() {
  if (process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1') {
    throw new Error('Unsigned artifacts may be staged locally but never uploaded');
  }
  const version = packageVersion();
  const tag = `v${version}`;
  const files = releaseFiles();
  if (files.length === 0) throw new Error(`No staged release files found in ${releaseDir}`);
  const sessionPath = path.join(releaseDir, '.build-session.json');
  let session;
  try {
    session = JSON.parse(fs.readFileSync(sessionPath, 'utf8'));
  } catch (error) {
    throw new Error(
      `Release build session is missing or invalid: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  if (typeof session.target !== 'string' || session.target.length === 0) {
    throw new Error('Release build session has no target');
  }
  const waitForDraft =
    process.argv.includes('--wait') || process.env.DEOX_RELEASE_DRAFT_MODE === 'wait';
  const buildEnv = {
    ...buildEnvironment(process.env),
    DEOX_RELEASE_TARGET: session.target,
  };

  run('node', ['scripts/release-session.cjs', 'verify', session.target], buildEnv);
  run('npm', ['run', 'release:verify'], buildEnv);
  run(
    'node',
    ['scripts/ensure-draft-release.cjs', ...(waitForDraft ? ['--wait'] : [])],
    buildEnv,
  );
  upload(tag, files, buildEnv);
  run('npm', ['run', 'release:verify:remote'], buildEnv);
}

function main() {
  if (process.argv.includes('--upload-only')) {
    uploadExisting();
    return;
  }
  const os = normalizeOs(process.argv[2] || '');
  const arch = normalizeArch(process.argv[3] || 'host', os);
  const target = TARGETS[os]?.[arch];
  if (!target) throw new Error(`No target triple for ${os}-${arch}`);

  const uploadRequested = process.argv.includes('--upload');
  const waitForDraft = process.argv.includes('--wait') || process.env.DEOX_RELEASE_DRAFT_MODE === 'wait';
  const unsigned = process.argv.includes('--unsigned') || process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1';
  if (unsigned && process.env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error('Unsigned staging requires DEOX_RELEASE_CONFIRM=YES explicitly');
  }
  if (uploadRequested && unsigned) {
    throw new Error('Unsigned artifacts may be staged locally but never uploaded');
  }
  const env = {
    ...process.env,
    DEOX_RELEASE_TARGET: target,
    DEOX_CHECKSUM_NAME: `SHA256SUMS-${os}-${arch}.txt`,
    ...(unsigned ? { DEOX_ALLOW_UNSIGNED_RELEASE: '1' } : {}),
  };
  if (
    process.env.DEOX_CHECKSUM_NAME &&
    process.env.DEOX_CHECKSUM_NAME !== env.DEOX_CHECKSUM_NAME
  ) {
    // Release path always uses the per-target canonical manifest; a legacy
    // global SHA256SUMS.txt override is ignored here (warn, do not fail, so
    // local staging stays usable).
    console.error(
      `warn: ignoring DEOX_CHECKSUM_NAME=${process.env.DEOX_CHECKSUM_NAME}; using canonical ${env.DEOX_CHECKSUM_NAME}`,
    );
  }
  const buildEnv = buildEnvironment(env);
  const signEnv = signingEnvironment(env, os);
  const gpgEnv = gpgEnvironment(env);

  fs.rmSync(releaseDir, { recursive: true, force: true });
  fs.mkdirSync(releaseDir, { recursive: true });
  // Quality checks and compilation never inherit signing credentials.
  run('npm', ['run', 'release:prepare'], buildEnv);
  run('rustup', ['target', 'add', '--toolchain', '1.98.1', target], buildEnv);
  run('cargo', ['build', '--release', '--locked', '--target', target], buildEnv);
  const binaryExtension = os === 'windows' ? '.exe' : '';
  const targetRoot = process.env.CARGO_TARGET_DIR || path.join(root, 'target');
  const targetReleaseDir = path.join(targetRoot, target, 'release');
  const binaryPaths = [
    path.join(targetReleaseDir, `deoxidizer${binaryExtension}`),
    path.join(targetReleaseDir, `deox${binaryExtension}`),
  ];
  if (os === 'darwin') {
    if (process.platform !== 'darwin') {
      throw new Error('macOS signing must run on macOS');
    }
    run('bash', ['scripts/macos-codesign.sh', ...binaryPaths], signEnv);
  }
  if (os === 'windows') {
    if (process.platform !== 'win32') {
      throw new Error('Windows Authenticode signing must run on Windows');
    }
    for (const binaryPath of binaryPaths) {
      run(
        'powershell.exe',
        ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/windows-artifact-sign.ps1', '-FilePath', binaryPath],
        signEnv,
      );
    }
    const installerName = `deoxidizer-v${packageVersion()}-windows-${arch}-setup.exe`;
    const buildDir = targetReleaseDir.replaceAll(path.sep, '\\');
    run(
      'makensis.exe',
      [
        `/DBUILD_DIR=${buildDir}`,
        '/DOUTPUT_DIR=release',
        `/DOUTPUT_NAME=${installerName}`,
        `/DVERSION=${packageVersion()}`,
        'installer.nsi',
      ],
      buildEnv,
    );
    run(
      'powershell.exe',
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        'scripts/windows-artifact-sign.ps1',
        '-FilePath',
        path.join(releaseDir, installerName),
      ],
      signEnv,
    );
  }
  run('bash', ['scripts/build-release.sh', '--target', target, '--skip-build'], buildEnv);
  if (os === 'windows') {
    const archivePath = path.join(releaseDir, `deoxidizer-v${packageVersion()}-windows-${arch}.zip`);
    run(
      'powershell.exe',
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        'scripts/verify-windows-authenticode.ps1',
        '-TargetReleaseDir',
        targetReleaseDir,
        '-Archives',
        archivePath,
        '-ExtraFiles',
        path.join(releaseDir, `deoxidizer-v${packageVersion()}-windows-${arch}-setup.exe`),
      ],
      signEnv,
    );
  }
  run('bash', ['scripts/gpg-sign.sh', 'release', ...(unsigned ? ['--allow-unsigned'] : [])], gpgEnv);
  run('npm', ['run', 'release:verify'], buildEnv);

  const version = packageVersion();
  if (!version) throw new Error('Cargo.toml has no version');
  const tag = `v${version}`;
  const files = releaseFiles();

  if (uploadRequested) {
    run(
      'node',
      ['scripts/ensure-draft-release.cjs', ...(waitForDraft ? ['--wait'] : [])],
      buildEnv,
    );
    upload(tag, files, buildEnv);
    run('npm', ['run', 'release:verify:remote'], buildEnv);
  } else {
    console.log('Artifacts staged locally. Re-run with --upload to publish through gh.');
  }
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ Release failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { TARGETS, normalizeArch, normalizeOs, buildEnvironment };
