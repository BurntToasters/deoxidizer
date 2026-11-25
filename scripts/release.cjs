'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { uploadReleaseAsset, withReleaseMutationLock } = require('./github-cli.cjs');
const { cargoVersion, validateVersion } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');
const stagingRoot = path.join(releaseDir, 'staging');
const SIGNING_ENV_KEYS = [
  'TAURI_SIGNING_PRIVATE_KEY_PATH',
  'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
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
  'DEOX_ALLOW_UNNOTARIZED',
];

const TARGETS = {
  // Static musl binaries run on every Linux distribution regardless of the
  // release VM's glibc version.
  linux: {
    x86_64: 'x86_64-unknown-linux-musl',
    aarch64: 'aarch64-unknown-linux-musl',
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

/**
 * Environment additions for building `target` on this host. Linux musl
 * builds compile `ring`'s C code with `musl-gcc` (Debian/Ubuntu package
 * `musl-tools`) and must run on a host of the same architecture; cross
 * builds fail early with a clear message instead of an obscure cc error.
 */
function targetBuildEnvironment(os, arch, env = process.env, host = { os: hostOs(), arch: hostArch() }) {
  if (os !== 'linux') return {};
  if (host.os !== 'linux' || host.arch !== arch) {
    throw new Error(
      `Linux ${arch} releases must be built on a Linux ${arch} host (got ${host.os} ${host.arch})`,
    );
  }
  const variable = `CC_${TARGETS.linux[arch].replaceAll('-', '_')}`;
  if (env[variable]) return {};
  const probe = spawnSync('musl-gcc', ['--version'], { stdio: 'ignore' });
  if (probe.error || probe.status !== 0) {
    throw new Error('musl-gcc is required for Linux release builds (install the musl-tools package)');
  }
  return { [variable]: 'musl-gcc' };
}

function windowsFileVersion(version) {
  return `${version.split('-', 1)[0]}.0`;
}

function releaseStageDirectory(target, baseDir = releaseDir) {
  if (!Object.values(TARGETS).some((arches) => Object.values(arches).includes(target))) {
    throw new Error(`Unsupported release staging target: ${target}`);
  }
  return path.join(baseDir, 'staging', target);
}

function acquireStageLock(stageDir) {
  const parent = path.dirname(stageDir);
  fs.mkdirSync(parent, { recursive: true });
  const lockPath = path.join(parent, `.${path.basename(stageDir)}.lock`);
  const owner = `${process.pid}:${Date.now()}\n`;
  let descriptor;
  try {
    descriptor = fs.openSync(lockPath, 'wx', 0o600);
    fs.writeSync(descriptor, owner);
  } catch (error) {
    if (descriptor !== undefined) {
      fs.closeSync(descriptor);
      try {
        fs.unlinkSync(lockPath);
      } catch {}
    }
    if (error?.code === 'EEXIST') {
      throw new Error(`Release staging is locked by another build: ${lockPath}`);
    }
    throw error;
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    try {
      fs.closeSync(descriptor);
    } finally {
      try {
        if (fs.readFileSync(lockPath, 'utf8') === owner) fs.unlinkSync(lockPath);
      } catch (error) {
        if (error?.code !== 'ENOENT') throw error;
      }
    }
  };
}

function ensureRealDirectory(directory) {
  if (fs.existsSync(directory)) {
    const stats = fs.lstatSync(directory);
    if (!stats.isDirectory() || stats.isSymbolicLink()) {
      throw new Error(`Release staging path is not a real directory: ${directory}`);
    }
    return;
  }
  fs.mkdirSync(directory, { recursive: true });
}

function npmReleaseInvocation(options = {}) {
  const { npmUpdateInvocation } = require('./npm-safe-update.cjs');
  const invocation = npmUpdateInvocation(options);
  return { command: invocation.command, args: invocation.prefixArgs };
}

function runNpm(args, env = process.env) {
  const invocation = npmReleaseInvocation({ env, platform: process.platform, execPath: process.execPath });
  run(invocation.command, [...invocation.args, ...args], env);
}

function sha256File(filePath) {
  const descriptor = fs.openSync(filePath, 'r');
  const hash = crypto.createHash('sha256');
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    while (true) {
      const read = fs.readSync(descriptor, buffer, 0, buffer.length, null);
      if (read === 0) break;
      hash.update(buffer.subarray(0, read));
    }
  } finally {
    fs.closeSync(descriptor);
  }
  return hash.digest('hex');
}

function run(command, args, env = process.env) {
  const { usesWindowsCmdShell } = require('./npm-safe-update.cjs');
  const result = spawnSync(command, args, {
    cwd: root,
    env,
    encoding: 'utf8',
    shell: usesWindowsCmdShell(command),
    windowsHide: true,
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

function verificationEnvironment(env, unsigned) {
  const sanitized = buildEnvironment(env);
  if (unsigned) sanitized.DEOX_ALLOW_UNSIGNED_RELEASE = '1';
  return sanitized;
}

function signingEnvironment(env, os) {
  const scoped = { ...env };
  delete scoped.GH_TOKEN;
  delete scoped.GITHUB_TOKEN;
  const keep = new Set();
  if (os === 'darwin') {
    for (const key of [
      'APPLE_SIGNING_IDENTITY',
      'APPLE_ID',
      'APPLE_PASSWORD',
      'APPLE_TEAM_ID',
      'APPLE_KEYCHAIN_PROFILE',
      'DEOX_ALLOW_UNNOTARIZED',
    ]) {
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

function minisignEnvironment(env) {
  const scoped = buildEnvironment(env);
  for (const key of ['TAURI_SIGNING_PRIVATE_KEY_PATH', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD']) {
    if (env[key] !== undefined) scoped[key] = env[key];
  }
  return scoped;
}

function releaseFiles(directory = releaseDir) {
  if (!fs.existsSync(directory)) return [];
  return fs.readdirSync(directory).flatMap((name) => {
    if (name.startsWith('.')) return [];
    const filePath = path.join(directory, name);
    const stats = fs.lstatSync(filePath);
    if (!stats.isFile()) throw new Error(`Staged release entry is not a regular file: ${name}`);
    return [filePath];
  });
}

function upload(tag, files, environment) {
  for (const filePath of files) {
    uploadReleaseAsset(tag, filePath, { environment });
    console.log(`uploaded ${path.basename(filePath)}`);
  }
}

function stagedTargets() {
  if (!fs.existsSync(stagingRoot)) return [];
  return fs.readdirSync(stagingRoot).filter((target) => {
    if (!Object.values(TARGETS).some((arches) => Object.values(arches).includes(target))) return false;
    const targetDir = path.join(stagingRoot, target);
    const stats = fs.lstatSync(targetDir);
    return stats.isDirectory() && !stats.isSymbolicLink() &&
      fs.existsSync(path.join(targetDir, '.build-session.json'));
  });
}

function uploadExisting() {
  if (process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1') {
    throw new Error('Unsigned artifacts may be staged locally but never uploaded');
  }
  const version = validateVersion(packageVersion());
  const tag = `v${version}`;
  const targetArgument = process.argv.indexOf('--target');
  const requestedTarget = targetArgument >= 0 ? process.argv[targetArgument + 1] : undefined;
  if (targetArgument >= 0 && !requestedTarget) throw new Error('--target requires a Rust target triple');
  const target =
    requestedTarget ||
    process.env.DEOX_RELEASE_TARGET ||
    (() => {
      const available = stagedTargets();
      if (available.length !== 1) {
        throw new Error(
          `Select one staged target with --target; found ${available.length} staged targets`,
        );
      }
      return available[0];
    })();
  const stageDir = releaseStageDirectory(target);
  ensureRealDirectory(releaseDir);
  ensureRealDirectory(stagingRoot);
  const releaseLock = acquireStageLock(stageDir);
  try {
    ensureRealDirectory(stageDir);
    const sessionPath = path.join(stageDir, '.build-session.json');
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
    if (session.target !== target) {
      throw new Error('Release staging target does not match its build session');
    }
    const files = releaseFiles(stageDir);
    if (files.length === 0) throw new Error(`No staged release files found in ${stageDir}`);
    const waitForDraft =
      process.argv.includes('--wait') || process.env.DEOX_RELEASE_DRAFT_MODE === 'wait';
    const buildEnv = {
      ...buildEnvironment(process.env),
      DEOX_RELEASE_TARGET: session.target,
      DEOX_RELEASE_STAGE_DIR: stageDir,
      DEOX_RELEASE_COMMIT: session.commit,
    };

    const uploadEnv = {
      ...buildEnv,
      ...(process.env.GH_TOKEN ? { GH_TOKEN: process.env.GH_TOKEN } : {}),
      ...(process.env.GITHUB_TOKEN ? { GITHUB_TOKEN: process.env.GITHUB_TOKEN } : {}),
    };
    run('node', ['scripts/release-session.cjs', 'verify', session.target], buildEnv);
    runNpm(['run', 'release:verify'], buildEnv);
    withReleaseMutationLock(tag, session.commit, () => {
      run(
        'node',
        ['scripts/ensure-draft-release.cjs', ...(waitForDraft ? ['--wait'] : [])],
        uploadEnv,
      );
      upload(tag, releaseFiles(stageDir), uploadEnv);
      runNpm(['run', 'release:verify:remote'], uploadEnv);
    }, { environment: uploadEnv });
  } finally {
    releaseLock();
  }
}

function writeLinuxProof(proofPath, target, commit, binaryPaths) {
  const artifacts = Object.fromEntries(
    binaryPaths.map((filePath) => [path.basename(filePath), sha256File(filePath)]),
  );
  fs.writeFileSync(
    proofPath,
    `${JSON.stringify(
      {
        schema_version: 1,
        platform: 'linux',
        target,
        commit,
        artifacts,
        native: { mechanism: 'none', status: 'not-required' },
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
}

function readStageSession(stageDir, target) {
  let session;
  try {
    session = JSON.parse(fs.readFileSync(path.join(stageDir, '.build-session.json'), 'utf8'));
  } catch (error) {
    throw new Error(
      `Release build session is missing or invalid: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  if (session.target !== target || !/^[0-9a-f]{40}$/i.test(String(session.commit || ''))) {
    throw new Error('Release build session target or commit is invalid');
  }
  return session;
}

function main() {
  if (process.argv.includes('--upload-only')) {
    uploadExisting();
    return;
  }
  // Reject malformed versions before deriving stage, installer, or asset names.
  const version = validateVersion(packageVersion());
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
  if (uploadRequested && os === 'darwin') {
    if (!process.env.APPLE_SIGNING_IDENTITY || process.env.APPLE_SIGNING_IDENTITY === '-') {
      throw new Error('Public macOS upload requires a Developer ID signing identity');
    }
    if (process.env.DEOX_ALLOW_UNNOTARIZED === '1') {
      throw new Error('Public macOS upload requires accepted notarization proof');
    }
  }
  if (uploadRequested && os === 'windows' && process.env.SKIP_WIN_CODESIGN === '1') {
    throw new Error('Public Windows upload requires verified Authenticode signatures');
  }
  if (os === 'darwin' && process.platform !== 'darwin') {
    throw new Error('macOS signing must run on macOS');
  }
  if (os === 'windows' && process.platform !== 'win32') {
    throw new Error('Windows Authenticode signing must run on Windows');
  }

  const stageDir = releaseStageDirectory(target);
  ensureRealDirectory(releaseDir);
  ensureRealDirectory(stagingRoot);
  const releaseLock = acquireStageLock(stageDir);
  try {
    const existing = fs.existsSync(stageDir) ? fs.lstatSync(stageDir) : null;
    if (existing && (!existing.isDirectory() || existing.isSymbolicLink())) {
      throw new Error(`Release target staging path is not a real directory: ${stageDir}`);
    }
    fs.rmSync(stageDir, { recursive: true, force: true });
    fs.mkdirSync(stageDir, { recursive: false });

    const env = {
      ...process.env,
      ...targetBuildEnvironment(os, arch),
      DEOX_RELEASE_TARGET: target,
      DEOX_RELEASE_STAGE_DIR: stageDir,
      DEOX_CHECKSUM_NAME: `SHA256SUMS-${os}-${arch}.txt`,
      ...(unsigned ? { DEOX_ALLOW_UNSIGNED_RELEASE: '1' } : {}),
    };
    if (process.env.DEOX_CHECKSUM_NAME && process.env.DEOX_CHECKSUM_NAME !== env.DEOX_CHECKSUM_NAME) {
      console.error(
        `warn: ignoring DEOX_CHECKSUM_NAME=${process.env.DEOX_CHECKSUM_NAME}; using canonical ${env.DEOX_CHECKSUM_NAME}`,
      );
    }
    const buildEnv = buildEnvironment(env);
    const signEnv = signingEnvironment(env, os);
    const minisignEnv = minisignEnvironment(env);

    // Quality checks and compilation never inherit signing credentials.
    runNpm(['run', 'release:prepare'], buildEnv);
    run('rustup', ['target', 'add', '--toolchain', '1.98.1', target], buildEnv);
    run('cargo', ['build', '--release', '--locked', '--target', target], buildEnv);
    const session = readStageSession(stageDir, target);
    const proofPath = path.join(stageDir, '.native-proof.json');
    const committedBuildEnv = { ...buildEnv, DEOX_RELEASE_COMMIT: session.commit };
    run('node', ['scripts/release-session.cjs', 'verify', target], committedBuildEnv);

    const binaryExtension = os === 'windows' ? '.exe' : '';
    const targetRoot = process.env.CARGO_TARGET_DIR || path.join(root, 'target');
    const targetReleaseDir = path.join(targetRoot, target, 'release');
    const binaryPaths = [
      path.join(targetReleaseDir, `deoxidizer${binaryExtension}`),
      path.join(targetReleaseDir, `deox${binaryExtension}`),
    ];
    if (os === 'linux') writeLinuxProof(proofPath, target, session.commit, binaryPaths);
    if (os === 'darwin') {
      run(
        'bash',
        [
          'scripts/macos-codesign.sh',
          '--proof-file',
          proofPath,
          '--target',
          target,
          '--commit',
          session.commit,
          ...binaryPaths,
        ],
        signEnv,
      );
    }

    let installerPath;
    if (os === 'windows') {
      for (const binaryPath of binaryPaths) {
        run(
          'powershell.exe',
          [
            '-NoProfile',
            '-ExecutionPolicy',
            'Bypass',
            '-File',
            'scripts/windows-artifact-sign.ps1',
            '-FilePath',
            binaryPath,
          ],
          signEnv,
        );
      }
      const installerName = `deoxidizer-v${version}-windows-${arch}-setup.exe`;
      installerPath = path.join(stageDir, installerName);
      const buildDir = targetReleaseDir.replaceAll(path.sep, '\\');
      const outputDir = stageDir.replaceAll(path.sep, '\\');
      run(
        'makensis.exe',
        [
          `/DBUILD_DIR=${buildDir}`,
          `/DOUTPUT_DIR=${outputDir}`,
          `/DOUTPUT_NAME=${installerName}`,
          `/DVERSION=${version}`,
          `/DFILE_VERSION=${windowsFileVersion(version)}`,
          'installer.nsi',
        ],
        committedBuildEnv,
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
          installerPath,
        ],
        signEnv,
      );
      // Record proof only after native signature, publisher, and RFC3161
      // timestamp checks succeed for both binaries and the installer.
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
          '-ExtraFiles',
          installerPath,
          '-ProofPath',
          proofPath,
          '-Target',
          target,
          '-Commit',
          session.commit,
        ],
        signEnv,
      );
    }

    run(
      'bash',
      [
        'scripts/build-release.sh',
        '--target',
        target,
        '--skip-build',
        '--output-dir',
        stageDir,
        '--proof-file',
        proofPath,
      ],
      committedBuildEnv,
    );
    if (os === 'windows') {
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
          '-ExtraFiles',
          installerPath,
        ],
        signEnv,
      );
    }
    run(
      'node',
      ['scripts/minisign-sign.cjs', stageDir, ...(unsigned ? ['--allow-unsigned'] : [])],
      minisignEnv,
    );
    run('node', ['scripts/release-session.cjs', 'verify', target], committedBuildEnv);
    runNpm(['run', 'release:verify'], verificationEnvironment(committedBuildEnv, unsigned));

    const tag = `v${version}`;
    const files = releaseFiles(stageDir);
    if (uploadRequested) {
      const uploadEnv = {
        ...committedBuildEnv,
        ...(process.env.GH_TOKEN ? { GH_TOKEN: process.env.GH_TOKEN } : {}),
        ...(process.env.GITHUB_TOKEN ? { GITHUB_TOKEN: process.env.GITHUB_TOKEN } : {}),
      };
      withReleaseMutationLock(tag, session.commit, () => {
        run(
          'node',
          ['scripts/ensure-draft-release.cjs', ...(waitForDraft ? ['--wait'] : [])],
          uploadEnv,
        );
        upload(tag, files, uploadEnv);
        runNpm(['run', 'release:verify:remote'], uploadEnv);
      }, { environment: uploadEnv });
    } else {
      console.log('Artifacts staged locally. Re-run with --upload to publish through gh.');
    }
  } finally {
    releaseLock();
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

module.exports = {
  TARGETS,
  normalizeArch,
  normalizeOs,
  buildEnvironment,
  targetBuildEnvironment,
  verificationEnvironment,
  windowsFileVersion,
  releaseStageDirectory,
  acquireStageLock,
  npmReleaseInvocation,
  minisignEnvironment,
};
