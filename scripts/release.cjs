'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { uploadReleaseAsset } = require('./github-cli.cjs');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');

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
  const version = fs
    .readFileSync(path.join(root, 'Cargo.toml'), 'utf8')
    .match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error('Cargo.toml has no version');
  return version;
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

function releaseFiles() {
  return fs
    .readdirSync(releaseDir)
    .filter((name) => !name.startsWith('.') && fs.statSync(path.join(releaseDir, name)).isFile())
    .map((name) => path.join(releaseDir, name));
}

function upload(tag, files) {
  for (const filePath of files) {
    uploadReleaseAsset(tag, filePath, { clobber: true });
    console.log(`uploaded ${path.basename(filePath)}`);
  }
}

function main() {
  const os = normalizeOs(process.argv[2] || '');
  const arch = normalizeArch(process.argv[3] || 'host', os);
  const target = TARGETS[os]?.[arch];
  if (!target) throw new Error(`No target triple for ${os}-${arch}`);

  const uploadRequested = process.argv.includes('--upload');
  const waitForDraft = process.argv.includes('--wait') || process.env.DEOX_RELEASE_DRAFT_MODE === 'wait';
  const unsigned = process.argv.includes('--unsigned') || process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1';
  if (uploadRequested && unsigned) {
    throw new Error('Unsigned artifacts may be staged locally but never uploaded');
  }
  const env = {
    ...process.env,
    DEOX_RELEASE_TARGET: target,
    DEOX_CHECKSUM_NAME: `SHA256SUMS-${os}-${arch}.txt`,
    ...(unsigned ? { DEOX_ALLOW_UNSIGNED_RELEASE: '1' } : {}),
  };

  fs.rmSync(releaseDir, { recursive: true, force: true });
  fs.mkdirSync(releaseDir, { recursive: true });
  run('npm', ['run', 'release:prepare'], env);
  run('rustup', ['target', 'add', '--toolchain', '1.98.0', target], env);
  run('cargo', ['build', '--release', '--locked', '--target', target], env);
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
    run('bash', ['scripts/macos-codesign.sh', ...binaryPaths], env);
  }
  if (os === 'windows') {
    if (process.platform !== 'win32') {
      throw new Error('Windows Authenticode signing must run on Windows');
    }
    for (const binaryPath of binaryPaths) {
      run(
        'powershell.exe',
        ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/windows-artifact-sign.ps1', '-FilePath', binaryPath],
        env,
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
        'installer.nsi',
      ],
      env,
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
      env,
    );
  }
  run('bash', ['scripts/build-release.sh', '--target', target, '--skip-build'], env);
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
      env,
    );
  }
  run('bash', ['scripts/gpg-sign.sh', 'release', ...(unsigned ? ['--allow-unsigned'] : [])], env);
  run('npm', ['run', 'release:verify'], env);

  const version = packageVersion();
  if (!version) throw new Error('Cargo.toml has no version');
  const tag = `v${version}`;
  const files = releaseFiles();

  if (uploadRequested) {
    run(
      'node',
      ['scripts/ensure-draft-release.cjs', ...(waitForDraft ? ['--wait'] : [])],
      env,
    );
    upload(tag, files);
    run('npm', ['run', 'release:verify:remote'], env);
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

module.exports = { TARGETS, normalizeArch, normalizeOs };
