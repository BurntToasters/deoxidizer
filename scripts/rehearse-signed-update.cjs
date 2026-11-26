'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { cargoVersion } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const maxArchiveBytes = 256 * 1024 * 1024;
const maxManifestBytes = 4 * 1024 * 1024;
const maxSignatureBytes = 64 * 1024;
const pendingName = '.deoxidizer-update-pending';
let offlineToolchain;

class UsageError extends Error {}

class UnsupportedNativeCheck extends Error {}

function usage() {
  return [
    'Usage: node scripts/rehearse-signed-update.cjs --stage-dir <path> [options]',
    '',
    'Offline rehearsal of the host-native signed candidate in a release stage.',
    'The pinned Minisign verifier, signed archive digest, native proof, and',
    'candidate CLI digests are checked before private disposable fixtures are staged.',
    'Each replaced file is confirmed by retained file identity (bigint device/inode).',
    '',
    'Options:',
    '  --stage-dir <path>    Candidate release/staging/<native-rust-target> directory (required)',
    '  --binary-dir <path>   Matching native target/release directory (defaults to target/<target>/release)',
    '  --output <path>       Transcript file under target/e2e-artifacts/ (defaults to a unique file)',
    '  -h, --help            Show this help',
    '',
    'No network, signing private key, live installation, or CLI bypass option is used.',
    'Exit codes: 0 complete, 1 failed rehearsal, 2 usage error, 3 native-only check unsupported.',
  ].join('\n');
}

function parseArgs(argv) {
  const options = { stageDir: null, binaryDir: null, output: null, help: false };
  const seen = new Set();
  const names = new Map([
    ['--stage-dir', 'stageDir'],
    ['--binary-dir', 'binaryDir'],
    ['--output', 'output'],
  ]);

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '-h' || argument === '--help') {
      options.help = true;
      continue;
    }
    const property = names.get(argument);
    if (!property) throw new UsageError(`Unknown option: ${argument}`);
    if (seen.has(argument)) throw new UsageError(`Option may be provided only once: ${argument}`);
    seen.add(argument);
    const value = argv[index + 1];
    if (!value || value.startsWith('--')) throw new UsageError(`${argument} requires a path`);
    options[property] = value;
    index += 1;
  }

  if (!options.help && !options.stageDir) throw new UsageError('--stage-dir is required');
  return options;
}

function nativeTarget() {
  const platform =
    process.platform === 'darwin'
      ? 'darwin'
      : process.platform === 'win32'
        ? 'windows'
        : process.platform === 'linux'
          ? 'linux'
          : null;
  const arch = process.arch === 'arm64' ? 'aarch64' : process.arch === 'x64' ? 'x86_64' : null;
  if (!platform || !arch) {
    throw new Error(`Unsupported native rehearsal host: ${process.platform}/${process.arch}`);
  }

  let target;
  if (platform === 'darwin') target = `${arch}-apple-darwin`;
  else if (platform === 'windows') target = `${arch}-pc-windows-msvc`;
  else target = `${arch}-unknown-linux-musl`;

  return {
    platform,
    arch,
    target,
    extension: platform === 'windows' ? 'zip' : 'tar.gz',
    manifestName: `SHA256SUMS-${platform}-${arch}.txt`,
    archiveName(version) {
      return `deoxidizer-v${version}-${platform}-${arch}.${this.extension}`;
    },
    binaryNames: platform === 'windows' ? ['deoxidizer.exe', 'deox.exe'] : ['deoxidizer', 'deox'],
  };
}

function artifactLocation(requested, target) {
  const artifactDirectory = path.join(root, 'target', 'e2e-artifacts');
  const fileName = requested
    ? path.basename(requested)
    : `signed-update-rehearsal-${target.target}-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
  if (!fileName || fileName === '.' || fileName === '..' || !fileName.endsWith('.json')) {
    throw new UsageError('--output must name a .json file under target/e2e-artifacts');
  }
  if (requested) {
    const resolved = path.resolve(root, requested);
    if (path.dirname(resolved) !== artifactDirectory) {
      throw new UsageError('--output must be directly under target/e2e-artifacts');
    }
    return { directory: artifactDirectory, file: resolved };
  }
  return { directory: artifactDirectory, file: path.join(artifactDirectory, fileName) };
}

function ensureArtifactDirectory(directory) {
  const targetDirectory = path.join(root, 'target');
  for (const current of [targetDirectory, directory]) {
    try {
      const stat = fs.lstatSync(current);
      if (!stat.isDirectory() || stat.isSymbolicLink()) {
        throw new Error(`Artifact directory is not a real directory: ${path.relative(root, current)}`);
      }
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      fs.mkdirSync(current, { mode: 0o700 });
      const stat = fs.lstatSync(current);
      if (!stat.isDirectory() || stat.isSymbolicLink()) {
        throw new Error(`Artifact directory was replaced during creation: ${path.relative(root, current)}`);
      }
    }
  }
}

function writeArtifact(location, report) {
  ensureArtifactDirectory(location.directory);
  try {
    fs.lstatSync(location.file);
    throw new Error(`Refusing to overwrite rehearsal artifact: ${path.relative(root, location.file)}`);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const bytes = Buffer.from(`${JSON.stringify(report, null, 2)}\n`, 'utf8');
  const descriptor = fs.openSync(location.file, 'wx', 0o600);
  try {
    fs.writeFileSync(descriptor, bytes);
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
  return sha256(bytes);
}

function assertRealDirectory(directory, label) {
  let stat;
  try {
    stat = fs.lstatSync(directory);
  } catch (error) {
    throw new Error(`${label} is unavailable: ${error.message}`);
  }
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error(`${label} must be a real directory`);
}

function readRegularFile(filePath, limit, label) {
  let before;
  try {
    before = fs.lstatSync(filePath);
  } catch (error) {
    throw new Error(`${label} is unavailable: ${error.message}`);
  }
  if (!before.isFile() || before.isSymbolicLink()) throw new Error(`${label} must be a regular non-symlink file`);
  if (before.size > limit) throw new Error(`${label} exceeds its ${limit}-byte limit`);

  const noFollow = process.platform === 'win32' ? 0 : fs.constants.O_NOFOLLOW || 0;
  let descriptor;
  try {
    descriptor = fs.openSync(filePath, fs.constants.O_RDONLY | noFollow);
    const opened = fs.fstatSync(descriptor);
    if (!opened.isFile() || opened.size !== before.size || opened.dev !== before.dev || opened.ino !== before.ino) {
      throw new Error(`${label} changed while it was opened`);
    }
    const openedPath = fs.lstatSync(filePath);
    if (openedPath.isSymbolicLink() || openedPath.dev !== opened.dev || openedPath.ino !== opened.ino) {
      throw new Error(`${label} changed to a symlink or different file while it was opened`);
    }
    const bytes = fs.readFileSync(descriptor);
    if (bytes.length > limit) throw new Error(`${label} exceeds its ${limit}-byte limit`);
    const after = fs.fstatSync(descriptor);
    if (after.size !== opened.size || after.dev !== opened.dev || after.ino !== opened.ino) {
      throw new Error(`${label} changed while it was read`);
    }
    return { bytes, mode: opened.mode & 0o777, size: opened.size };
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
}

function sha256(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

function toolEnvironment(extra = {}) {
  const env = {};
  for (const name of [
    'PATH',
    'HOME',
    'USERPROFILE',
    'SystemRoot',
    'WINDIR',
    'PATHEXT',
    'TEMP',
    'TMP',
    'TMPDIR',
    'LANG',
    'LC_ALL',
    'TERM',
    'CARGO_HOME',
    'RUSTUP_HOME',
  ]) {
    if (process.env[name]) env[name] = process.env[name];
  }
  return { ...env, ...extra };
}

function redactDiagnostic(value) {
  return cleanOutput(value)
    .replace(/(https?:\/\/)[^/@\s:]+:[^/@\s]+@/gi, '$1[redacted]@')
    .replace(/\b(password|token|secret|private[_-]?key)=\S+/gi, '$1=[redacted]');
}

function parseManifest(manifestBytes) {
  const text = manifestBytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(manifestBytes)) throw new Error('Checksum manifest is not valid UTF-8');
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = line.match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
    if (!match || !match[2].trim()) throw new Error(`Invalid checksum manifest line: ${line}`);
    const name = match[2].trim();
    if (entries.has(name)) throw new Error(`Duplicate checksum entry: ${name}`);
    entries.set(name, match[1].toLowerCase());
  }
  return entries;
}

function runOfflineVerifier(example, args, label) {
  if (!offlineToolchain) {
    const toolchainText = fs.readFileSync(path.join(root, 'rust-toolchain.toml'), 'utf8');
    const match = toolchainText.match(/^\s*channel\s*=\s*"([^"]+)"/m);
    if (!match) throw new Error('Pinned Rust toolchain channel is missing');
    const channel = match[1];
    const installed = spawnSync('rustup', ['toolchain', 'list'], {
      cwd: root,
      env: toolEnvironment(),
      encoding: 'utf8',
      timeout: 10_000,
      maxBuffer: 64 * 1024,
    });
    if (installed.error || installed.status !== 0) {
      throw new Error(`Cannot confirm the pinned Rust toolchain is installed offline${installed.error ? `: ${installed.error.message}` : ''}`);
    }
    const installedNames = String(installed.stdout || '')
      .split(/\r?\n/)
      .map((line) => line.trim().split(/\s+/, 1)[0])
      .filter(Boolean);
    if (!installedNames.some((name) => name === channel || name.startsWith(`${channel}-`))) {
      throw new Error(`Pinned Rust toolchain ${channel} is not installed; refusing any network installation`);
    }
    offlineToolchain = channel;
  }
  const targetDirectory = path.join(root, 'target');
  try {
    const stat = fs.lstatSync(targetDirectory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('target is not a real directory');
  } catch (error) {
    if (error.code !== 'ENOENT') throw new Error(`Cannot run offline verifier safely: ${error.message}`);
  }
  const env = toolEnvironment({
    CARGO_NET_OFFLINE: 'true',
    CARGO_TARGET_DIR: targetDirectory,
    RUSTUP_TOOLCHAIN: offlineToolchain,
  });
  const result = spawnSync(
    'cargo',
    ['run', '--quiet', '--locked', '--offline', '--example', example, '--', ...args],
    { cwd: root, env, encoding: 'utf8', timeout: 180_000, maxBuffer: 4 * 1024 * 1024 },
  );
  if (result.error || result.status !== 0) {
    const detail = redactDiagnostic(String(result.stderr || '').split(/\r?\n/).slice(-8).join('\n')).slice(0, 1800);
    throw new Error(
      `${label} failed${result.error ? `: ${result.error.message}` : ''}${detail ? `: ${detail}` : ''}`,
    );
  }
  return String(result.stdout || '').trim();
}

function validateNativeProof(proof, target, binaries) {
  const expectedPlatform = target.platform;
  if (!proof || proof.schema_version !== 1) throw new Error('Candidate archive proof is missing or unsupported');
  if (proof.platform !== expectedPlatform || proof.target !== target.target) {
    throw new Error('Candidate archive native proof does not match this host');
  }
  if (!/^[a-f0-9]{40}$/i.test(String(proof.commit || ''))) {
    throw new Error('Candidate archive proof has no full commit identity');
  }
  const expectedArtifacts = [...target.binaryNames].sort();
  const actualArtifacts = Object.keys(proof.artifacts || {}).sort();
  if (JSON.stringify(actualArtifacts) !== JSON.stringify(expectedArtifacts)) {
    throw new Error('Candidate archive proof does not name exactly both invokers');
  }
  for (const name of expectedArtifacts) {
    if (String(proof.artifacts[name]).toLowerCase() !== binaries[name].sha256) {
      throw new Error(`Candidate archive proof digest does not match ${name}`);
    }
  }

  const native = proof.native || {};
  if (target.platform === 'darwin') {
    if (
      native.mechanism !== 'codesign' ||
      native.signature_verified !== true ||
      native.timestamp_verified !== true ||
      native.notarization?.status !== 'Accepted'
    ) {
      throw new Error('Candidate macOS proof does not confirm Developer ID signing and accepted notarization');
    }
  } else if (target.platform === 'windows') {
    if (
      native.mechanism !== 'authenticode' ||
      native.signature_verified !== true ||
      native.timestamp_verified !== true
    ) {
      throw new Error('Candidate Windows proof does not confirm Authenticode and timestamp verification');
    }
  } else if (native.mechanism !== 'none' || native.status !== 'not-required') {
    throw new Error('Candidate Linux proof has unexpected native signing metadata');
  }

  return {
    platform: proof.platform,
    target: proof.target,
    commit: proof.commit,
    artifacts: proof.artifacts,
    native: {
      mechanism: native.mechanism,
      signature_verified: native.signature_verified ?? null,
      timestamp_verified: native.timestamp_verified ?? null,
      status: native.status ?? null,
      notarization_status: native.notarization?.status ?? null,
    },
  };
}

function verifyCandidate(options, target) {
  const stageDirectory = path.resolve(root, options.stageDir);
  if (path.basename(stageDirectory) !== target.target) {
    throw new Error(`--stage-dir must name the native candidate target ${target.target}`);
  }
  assertRealDirectory(stageDirectory, 'Candidate stage');

  const version = cargoVersion(fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'));
  const archiveName = target.archiveName(version);
  const manifestName = target.manifestName;
  const archivePath = path.join(stageDirectory, archiveName);
  const manifestPath = path.join(stageDirectory, manifestName);
  const signaturePath = `${manifestPath}.minisig`;
  const manifestRecord = readRegularFile(manifestPath, maxManifestBytes, 'Target checksum manifest');
  const signatureRecord = readRegularFile(signaturePath, maxSignatureBytes, 'Raw Minisign sidecar');
  const archiveRecord = readRegularFile(archivePath, maxArchiveBytes, 'Candidate archive');

  runOfflineVerifier(
    'verify_release_manifest',
    [manifestPath, signaturePath],
    'Pinned Minisign manifest verification',
  );

  const entries = parseManifest(manifestRecord.bytes);
  const manifestDigest = entries.get(archiveName);
  if (!manifestDigest) throw new Error(`Signed manifest does not contain ${archiveName}`);
  const archiveDigest = sha256(archiveRecord.bytes);
  if (archiveDigest !== manifestDigest) {
    throw new Error(`Candidate archive SHA-256 mismatch: expected ${manifestDigest}, got ${archiveDigest}`);
  }

  const archiveOutput = runOfflineVerifier(
    'verify_release_archive',
    ['--json', '--require-proof', archivePath],
    'Production archive verification',
  );
  let archiveReport;
  try {
    archiveReport = JSON.parse(archiveOutput);
  } catch (error) {
    throw new Error(`Production archive verifier returned invalid JSON: ${error.message}`);
  }
  if (sha256(readRegularFile(archivePath, maxArchiveBytes, 'Candidate archive recheck').bytes) !== archiveDigest) {
    throw new Error('Candidate archive changed during verification');
  }
  if (sha256(readRegularFile(manifestPath, maxManifestBytes, 'Target checksum manifest recheck').bytes) !== sha256(manifestRecord.bytes) ||
      sha256(readRegularFile(signaturePath, maxSignatureBytes, 'Raw Minisign sidecar recheck').bytes) !== sha256(signatureRecord.bytes)) {
    throw new Error('Signed manifest inputs changed during verification');
  }

  const reportedBinaries = archiveReport.binaries || {};
  const binaries = {};
  for (const name of target.binaryNames) {
    const digest = String(reportedBinaries[name] || '').toLowerCase();
    if (!/^[a-f0-9]{64}$/.test(digest)) throw new Error(`Archive verifier did not report ${name}`);
    binaries[name] = { sha256: digest };
  }
  const nativeProof = validateNativeProof(archiveReport.proof, target, binaries);

  const binaryDirectory = options.binaryDir
    ? path.resolve(root, options.binaryDir)
    : path.join(root, 'target', target.target, 'release');
  assertRealDirectory(binaryDirectory, 'Candidate native binary directory');
  for (const name of target.binaryNames) {
    const binaryRecord = readRegularFile(path.join(binaryDirectory, name), maxArchiveBytes, `Candidate CLI ${name}`);
    const digest = sha256(binaryRecord.bytes);
    if (digest !== binaries[name].sha256) {
      throw new Error(`Candidate CLI ${name} does not match its signed archive digest`);
    }
    binaries[name].mode = binaryRecord.mode || 0o755;
    binaries[name].bytes = binaryRecord.bytes;
  }

  return {
    version,
    target,
    stageName: path.basename(stageDirectory),
    archiveName,
    manifestName,
    signatureName: `${manifestName}.minisig`,
    archive: archiveRecord.bytes,
    manifest: manifestRecord.bytes,
    signature: signatureRecord.bytes,
    archiveSha256: archiveDigest,
    manifestSha256: sha256(manifestRecord.bytes),
    signatureSha256: sha256(signatureRecord.bytes),
    manifestArchiveSha256: manifestDigest,
    binaries,
    proof: nativeProof,
  };
}

function makePrivateFixtureRoot() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-signed-update-rehearsal-'));
  if (process.platform !== 'win32') fs.chmodSync(directory, 0o700);
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Private fixture root is not a real directory');
  return directory;
}

function writeNewFile(filePath, bytes, mode) {
  const descriptor = fs.openSync(filePath, 'wx', mode);
  try {
    fs.writeFileSync(descriptor, bytes);
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
}

function removeFixtureDirectory(directory) {
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Fixture install path changed type before cleanup');
  fs.rmSync(directory, { recursive: true, force: false });
}

function seedFixture(fixtureRoot, label, candidate, mutation = null) {
  const installDirectory = path.join(fixtureRoot, label);
  fs.mkdirSync(installDirectory, { mode: 0o700 });
  for (const name of candidate.target.binaryNames) {
    const bytes = candidate.binaries[name].bytes;
    writeNewFile(path.join(installDirectory, name), bytes, candidate.binaries[name].mode || 0o755);
    if (process.platform !== 'win32') {
      fs.chmodSync(path.join(installDirectory, name), candidate.binaries[name].mode || 0o755);
    }
  }

  const pending = path.join(installDirectory, pendingName);
  fs.mkdirSync(pending, { mode: 0o700 });
  writeNewFile(path.join(pending, 'archive'), candidate.archive, 0o600);
  writeNewFile(path.join(pending, 'manifest'), candidate.manifest, 0o600);
  writeNewFile(path.join(pending, 'signature.minisig'), candidate.signature, 0o600);
  writeNewFile(path.join(pending, 'asset-name'), Buffer.from(candidate.archiveName, 'utf8'), 0o600);

  if (mutation === 'archive-hash') {
    const tampered = Buffer.from(candidate.archive);
    if (tampered.length === 0) throw new Error('Candidate archive is empty');
    tampered[tampered.length - 1] ^= 0x01;
    fs.writeFileSync(path.join(pending, 'archive'), tampered);
  } else if (mutation === 'signature') {
    const tampered = Buffer.from(candidate.signature);
    const firstLineEnd = tampered.indexOf(0x0a);
    let encodedByte = firstLineEnd + 1;
    while (encodedByte < tampered.length && (tampered[encodedByte] === 0x0a || tampered[encodedByte] === 0x0d)) {
      encodedByte += 1;
    }
    if (firstLineEnd < 0 || encodedByte >= tampered.length) throw new Error('Candidate signature has no encoded payload');
    tampered[encodedByte] = tampered[encodedByte] === 0x41 ? 0x42 : 0x41;
    fs.writeFileSync(path.join(pending, 'signature.minisig'), tampered);
  }

  const before = {};
  for (const name of candidate.target.binaryNames) {
    before[name] = sha256(readRegularFile(path.join(installDirectory, name), maxArchiveBytes, name).bytes);
    if (before[name] !== candidate.binaries[name].sha256) throw new Error(`Fixture original ${name} is not the verified candidate CLI`);
  }
  return { installDirectory, pending, before };
}

function childEnvironment(homeDirectory, tempDirectory) {
  const env = {};
  for (const name of ['PATH', 'SystemRoot', 'WINDIR', 'PATHEXT', 'LANG', 'LC_ALL', 'TERM']) {
    if (process.env[name]) env[name] = process.env[name];
  }
  env.HOME = homeDirectory;
  env.USERPROFILE = homeDirectory;
  env.APPDATA = path.join(homeDirectory, 'AppData', 'Roaming');
  env.LOCALAPPDATA = path.join(homeDirectory, 'AppData', 'Local');
  env.TEMP = tempDirectory;
  env.TMP = tempDirectory;
  env.TMPDIR = tempDirectory;
  // `run_update` honors these proxy variables. Any unexpected network path is
  // pinned to loopback, and NO_PROXY is cleared so the host cannot bypass it.
  for (const name of ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']) {
    env[name] = 'http://127.0.0.1:9';
  }
  env.NO_PROXY = '';
  env.no_proxy = '';
  return env;
}

function runCli(installDirectory, invoker, fixtureRoot) {
  const executable = path.join(installDirectory, invoker);
  const homeDirectory = path.join(fixtureRoot, 'home');
  const tempDirectory = path.join(fixtureRoot, 'tmp');
  fs.mkdirSync(homeDirectory, { mode: 0o700, recursive: true });
  fs.mkdirSync(tempDirectory, { mode: 0o700, recursive: true });
  const result = spawnSync(executable, ['--update'], {
    cwd: installDirectory,
    env: childEnvironment(homeDirectory, tempDirectory),
    encoding: 'utf8',
    timeout: 20_000,
    maxBuffer: 2 * 1024 * 1024,
    windowsHide: true,
  });
  const stdout = cleanOutput(result.stdout || '');
  const stderr = cleanOutput(result.stderr || '');
  if (result.error) throw new Error(`${invoker} could not complete the offline fixture invocation: ${result.error.message}`);
  return { exitCode: result.status, stdout, stderr };
}

function cleanOutput(value) {
  return String(value)
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .trim()
    .slice(0, 6000);
}

function fileDigests(installDirectory, binaryNames) {
  return Object.fromEntries(
    binaryNames.map((name) => [
      name,
      sha256(readRegularFile(path.join(installDirectory, name), maxArchiveBytes, name).bytes),
    ]),
  );
}

function identityFromStat(stat, label) {
  if (!stat.isFile() || stat.isSymbolicLink()) throw new Error(`${label} is not a regular file for identity observation`);
  if (stat.dev === 0n || stat.ino === 0n) {
    throw new Error(`${label} filesystem does not expose usable bigint device/inode identity`);
  }
  return { device: stat.dev.toString(), inode: stat.ino.toString() };
}

function fileIdentity(filePath, label) {
  return identityFromStat(fs.lstatSync(filePath, { bigint: true }), label);
}

function sameIdentity(left, right) {
  return left.device === right.device && left.inode === right.inode;
}

async function holdFileIdentities(installDirectory, binaryNames, fixtureRoot, lockedName = null) {
  const paths = Object.fromEntries(
    binaryNames.map((name) => [name, path.join(installDirectory, name)]),
  );
  const before = Object.fromEntries(
    binaryNames.map((name) => [name, fileIdentity(paths[name], name)]),
  );
  const descriptors = [];
  let windowsHold;

  if (process.platform === 'win32') {
    const lockPath = lockedName ? paths[lockedName] : null;
    windowsHold = await acquireWindowsFileHolds(Object.values(paths), lockPath, fixtureRoot);
    for (const name of binaryNames) {
      if (!sameIdentity(before[name], fileIdentity(paths[name], name))) {
        await windowsHold.release();
        throw new Error(`${name} changed while Windows identity handles were acquired`);
      }
    }
  } else {
    try {
      for (const name of binaryNames) {
        const descriptor = fs.openSync(paths[name], 'r');
        descriptors.push(descriptor);
        const held = identityFromStat(fs.fstatSync(descriptor, { bigint: true }), name);
        if (!sameIdentity(before[name], held)) throw new Error(`${name} changed while identity handle was acquired`);
      }
    } catch (error) {
      for (const descriptor of descriptors) fs.closeSync(descriptor);
      throw error;
    }
  }

  return {
    before,
    current() {
      return Object.fromEntries(binaryNames.map((name) => [name, fileIdentity(paths[name], name)]));
    },
    async release() {
      if (windowsHold) await windowsHold.release();
      for (const descriptor of descriptors.splice(0)) fs.closeSync(descriptor);
    },
    method:
      process.platform === 'win32'
        ? 'bigint dev/ino with read-only Windows handles sharing delete'
        : 'bigint dev/ino with original read-only file descriptors held open',
  };
}

function pendingExists(pending) {
  try {
    const stat = fs.lstatSync(pending);
    return stat.isDirectory() && !stat.isSymbolicLink();
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

function assertOriginalsUnchanged(before, after, label) {
  for (const [name, digest] of Object.entries(before)) {
    if (after[name] !== digest) throw new Error(`${label} changed original fixture binary ${name}`);
  }
}

async function positiveRecovery(fixtureRoot, candidate, invoker) {
  const fixture = seedFixture(fixtureRoot, `positive-${path.parse(invoker).name}`, candidate);
  let identities;
  try {
    identities = await holdFileIdentities(fixture.installDirectory, candidate.target.binaryNames, fixtureRoot);
    const result = runCli(fixture.installDirectory, invoker, fixtureRoot);
    if (result.exitCode !== 0 || !result.stdout.includes('Recovered the pending update for both invokers.')) {
      throw new Error(`${invoker} did not complete valid pending recovery: ${result.stderr || result.stdout}`);
    }
    const after = fileDigests(fixture.installDirectory, candidate.target.binaryNames);
    const afterIdentity = identities.current();
    for (const name of candidate.target.binaryNames) {
      if (after[name] !== candidate.binaries[name].sha256) {
        throw new Error(`Valid recovery did not install the signed archive digest for ${name}`);
      }
      if (sameIdentity(identities.before[name], afterIdentity[name])) {
        throw new Error(`Valid recovery did not replace ${name}; file identity is unchanged`);
      }
    }
    if (pendingExists(fixture.pending)) throw new Error('Valid recovery did not remove its completed pending package');
    return {
      invoker,
      exit_code: result.exitCode,
      branch: 'pending-recovery-before-release-check',
      identity_observation: identities.method,
      before_sha256: fixture.before,
      after_sha256: after,
      before_identity: identities.before,
      after_identity: afterIdentity,
      replaced: Object.fromEntries(candidate.target.binaryNames.map((name) => [name, true])),
      pending_removed: true,
      stdout: result.stdout,
      stderr: result.stderr,
    };
  } finally {
    if (identities) await identities.release();
    removeFixtureDirectory(fixture.installDirectory);
  }
}

async function rejectionRecovery(fixtureRoot, candidate, invoker, mutation) {
  const fixture = seedFixture(fixtureRoot, `reject-${mutation}-${path.parse(invoker).name}`, candidate, mutation);
  let identities;
  try {
    identities = await holdFileIdentities(fixture.installDirectory, candidate.target.binaryNames, fixtureRoot);
    const result = runCli(fixture.installDirectory, invoker, fixtureRoot);
    const combined = `${result.stdout}\n${result.stderr}`;
    if (result.exitCode === 0 || !combined.includes('Pending update recovery failed:')) {
      throw new Error(`${invoker} did not reject the ${mutation} fixture in pending recovery`);
    }
    const expected = mutation === 'archive-hash' ? /checksum/i : /signature|verification/i;
    if (!expected.test(combined)) throw new Error(`${mutation} rejection did not identify the failed verifier`);
    const after = fileDigests(fixture.installDirectory, candidate.target.binaryNames);
    const afterIdentity = identities.current();
    assertOriginalsUnchanged(fixture.before, after, `${mutation} rejection`);
    for (const name of candidate.target.binaryNames) {
      if (!sameIdentity(identities.before[name], afterIdentity[name])) {
        throw new Error(`${mutation} rejection replaced original fixture binary ${name}`);
      }
    }
    if (!pendingExists(fixture.pending)) throw new Error(`${mutation} rejection discarded its recoverable package`);
    return {
      invoker,
      mutation,
      exit_code: result.exitCode,
      branch: 'pending-recovery-before-release-check',
      identity_observation: identities.method,
      originals_unchanged: true,
      before_sha256: fixture.before,
      after_sha256: after,
      before_identity: identities.before,
      after_identity: afterIdentity,
      identities_unchanged: true,
      pending_retained: true,
      stdout: result.stdout,
      stderr: result.stderr,
    };
  } finally {
    if (identities) await identities.release();
    removeFixtureDirectory(fixture.installDirectory);
  }
}

function waitForChildExit(child, timeoutMs) {
  return new Promise((resolve, reject) => {
    if (child.exitCode !== null) return resolve(child.exitCode);
    const timer = setTimeout(() => reject(new Error('Windows identity handle helper did not exit')), timeoutMs);
    child.once('exit', (code) => {
      clearTimeout(timer);
      resolve(code);
    });
    child.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function acquireWindowsFileHolds(filePaths, lockedPath, fixtureRoot) {
  const scriptPath = path.join(
    fixtureRoot,
    `hold-file-identities-${crypto.randomBytes(8).toString('hex')}.ps1`,
  );
  const homeDirectory = path.join(fixtureRoot, 'home');
  const tempDirectory = path.join(fixtureRoot, 'tmp');
  fs.mkdirSync(homeDirectory, { mode: 0o700, recursive: true });
  fs.mkdirSync(tempDirectory, { mode: 0o700, recursive: true });
  const script = [
    'param([Parameter(Mandatory=$true)][string]$PathsJson, [string]$LockedPath)',
    '$paths = ConvertFrom-Json -InputObject $PathsJson',
    "$handles = New-Object 'System.Collections.Generic.List[System.IO.FileStream]'",
    'foreach ($candidatePath in $paths) {',
    '  $share = [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete',
    '  if ($LockedPath -and [string]::Equals($candidatePath, $LockedPath, [System.StringComparison]::OrdinalIgnoreCase)) { $share = [System.IO.FileShare]::ReadWrite }',
    '  $handles.Add([System.IO.File]::Open($candidatePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, $share))',
    '}',
    '[Console]::Out.WriteLine("READY")',
    '[Console]::Out.Flush()',
    '[void][Console]::In.ReadLine()',
    'foreach ($handle in $handles) { $handle.Dispose() }',
    '',
  ].join('\r\n');
  fs.writeFileSync(scriptPath, script, { encoding: 'ascii', flag: 'wx', mode: 0o600 });

  return new Promise((resolve, reject) => {
    let output = '';
    let ready = false;
    let settled = false;
    let timer;
    let child;
    const fail = (error) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      try {
        child?.kill();
      } catch {}
      reject(new UnsupportedNativeCheck(`Windows retained file handles could not be established: ${error.message}`));
    };
    try {
      child = spawn(
        'powershell.exe',
        ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', scriptPath, JSON.stringify(filePaths), lockedPath || ''],
        {
        cwd: fixtureRoot,
        env: childEnvironment(homeDirectory, tempDirectory),
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
        },
      );
    } catch (error) {
      fail(error);
      return;
    }
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8');
    });
    child.stdout.on('data', (chunk) => {
      output += chunk.toString('utf8');
      if (!ready && output.split(/\r?\n/).includes('READY')) {
        ready = true;
        settled = true;
        if (timer) clearTimeout(timer);
        resolve({
          release: async () => {
            child.stdin.end('\n');
            const code = await waitForChildExit(child, 5000);
            if (code !== 0) throw new Error(`Windows sibling lock helper exited with status ${code}`);
          },
        });
      }
    });
    child.once('error', (error) => {
      if (!ready) fail(error);
    });
    child.once('exit', (code) => {
      if (!ready) fail(new Error(`PowerShell exited ${code}: ${cleanOutput(stderr)}`));
    });
    timer = setTimeout(() => fail(new Error('PowerShell did not report all file handles within 10 seconds')), 10_000);
  });
}

async function interruptedRecoveryAndRetry(fixtureRoot, candidate) {
  const primary = candidate.target.binaryNames[0];
  const sibling = candidate.target.binaryNames[1];
  const fixture = seedFixture(fixtureRoot, 'sibling-interruption', candidate);
  try {
    let firstIdentities;
    try {
      firstIdentities = await holdFileIdentities(
        fixture.installDirectory,
        candidate.target.binaryNames,
        fixtureRoot,
        sibling,
      );
    } catch (error) {
      if (error instanceof UnsupportedNativeCheck) throw error;
      throw new Error(`Could not prepare Windows identity and sibling lock handles: ${error.message}`);
    }

    let first;
    let afterInterruptionIdentity;
    try {
      first = runCli(fixture.installDirectory, primary, fixtureRoot);
      afterInterruptionIdentity = firstIdentities.current();
    } finally {
      await firstIdentities.release();
    }
    const firstOutput = `${first.stdout}\n${first.stderr}`;
    if (first.exitCode === 0 || !firstOutput.includes('Pending update recovery failed: Failed to replace sibling executable:')) {
      throw new Error(`Locked sibling did not interrupt replacement after primary install: ${first.stderr || first.stdout}`);
    }
    const afterInterruption = fileDigests(fixture.installDirectory, candidate.target.binaryNames);
    if (afterInterruption[primary] !== candidate.binaries[primary].sha256) {
      throw new Error('Interrupted recovery did not install the primary archive binary');
    }
    if (afterInterruption[sibling] !== fixture.before[sibling]) {
      throw new Error('Interrupted recovery changed the locked sibling binary');
    }
    if (sameIdentity(firstIdentities.before[primary], afterInterruptionIdentity[primary])) {
      throw new Error('Interrupted recovery did not replace the primary file identity');
    }
    if (!sameIdentity(firstIdentities.before[sibling], afterInterruptionIdentity[sibling])) {
      throw new Error('Interrupted recovery replaced the locked sibling file identity');
    }
    if (!pendingExists(fixture.pending)) throw new Error('Interrupted recovery discarded its pending package');

    const retryIdentities = await holdFileIdentities(fixture.installDirectory, candidate.target.binaryNames, fixtureRoot);
    let retry;
    let afterRetryIdentity;
    try {
      retry = runCli(fixture.installDirectory, sibling, fixtureRoot);
      afterRetryIdentity = retryIdentities.current();
    } finally {
      await retryIdentities.release();
    }
    if (retry.exitCode !== 0 || !retry.stdout.includes('Recovered the pending update for both invokers.')) {
      throw new Error(`Same-version recovery retry through ${sibling} failed: ${retry.stderr || retry.stdout}`);
    }
    const afterRetry = fileDigests(fixture.installDirectory, candidate.target.binaryNames);
    for (const name of candidate.target.binaryNames) {
      if (afterRetry[name] !== candidate.binaries[name].sha256) {
        throw new Error(`Recovery retry did not install the signed archive digest for ${name}`);
      }
      if (sameIdentity(retryIdentities.before[name], afterRetryIdentity[name])) {
        throw new Error(`Recovery retry did not replace ${name}; file identity is unchanged`);
      }
    }
    if (pendingExists(fixture.pending)) throw new Error('Successful sibling retry did not remove the pending package');
    return {
      status: 'passed',
      locking_method: 'PowerShell FileShare.ReadWrite handle denies delete sharing',
      first_invoker: primary,
      first_exit_code: first.exitCode,
      identity_observation: retryIdentities.method,
      primary_updated_before_sibling_failure: true,
      sibling_unchanged_during_failure: true,
      first_before_identity: firstIdentities.before,
      first_after_identity: afterInterruptionIdentity,
      pending_retained_after_interruption: true,
      retry_invoker: sibling,
      retry_exit_code: retry.exitCode,
      after_interruption_sha256: afterInterruption,
      after_retry_sha256: afterRetry,
      retry_before_identity: retryIdentities.before,
      retry_after_identity: afterRetryIdentity,
      replaced_on_retry: Object.fromEntries(candidate.target.binaryNames.map((name) => [name, true])),
      pending_removed_after_retry: true,
      first_stdout: first.stdout,
      first_stderr: first.stderr,
      retry_stdout: retry.stdout,
      retry_stderr: retry.stderr,
    };
  } finally {
    removeFixtureDirectory(fixture.installDirectory);
  }
}

function publicCandidateRecord(candidate) {
  return {
    version: candidate.version,
    target: candidate.target.target,
    stage: candidate.stageName,
    archive_name: candidate.archiveName,
    archive_size: candidate.archive.length,
    archive_sha256: candidate.archiveSha256,
    manifest_name: candidate.manifestName,
    manifest_sha256: candidate.manifestSha256,
    manifest_archive_sha256: candidate.manifestArchiveSha256,
    signature_name: candidate.signatureName,
    signature_sha256: candidate.signatureSha256,
    binaries: Object.fromEntries(
      Object.entries(candidate.binaries).map(([name, value]) => [name, value.sha256]),
    ),
    native_proof: candidate.proof,
  };
}

async function runRehearsal(options, target, output) {
  const report = {
    schema_version: 1,
    rehearsal: 'offline-signed-pending-update',
    status: 'running',
    started_at_utc: new Date().toISOString(),
    host: { platform: process.platform, arch: process.arch },
    target: target.target,
    runtime_network_guard: 'HTTP_PROXY, HTTPS_PROXY, and ALL_PROXY point to 127.0.0.1:9; NO_PROXY is cleared.',
    checks: {},
  };
  let fixtureRoot;
  let exitCode = 1;
  try {
    const candidate = verifyCandidate(options, target);
    report.candidate = publicCandidateRecord(candidate);

    fixtureRoot = makePrivateFixtureRoot();
    report.checks.positive_recovery = [];
    report.checks.hash_rejection = [];
    report.checks.signature_rejection = [];
    for (const invoker of candidate.target.binaryNames) {
      report.checks.positive_recovery.push(await positiveRecovery(fixtureRoot, candidate, invoker));
      report.checks.hash_rejection.push(await rejectionRecovery(fixtureRoot, candidate, invoker, 'archive-hash'));
      report.checks.signature_rejection.push(await rejectionRecovery(fixtureRoot, candidate, invoker, 'signature'));
    }

    if (process.platform === 'win32') {
      try {
        report.checks.sibling_interruption = await interruptedRecoveryAndRetry(fixtureRoot, candidate);
      } catch (error) {
        if (!(error instanceof UnsupportedNativeCheck)) throw error;
        report.checks.sibling_interruption = { status: 'unsupported', reason: error.message };
        report.status = 'partial';
        report.unfinished = 'A native Windows fixture must demonstrate sibling interruption and same-version retry.';
        exitCode = 3;
      }
    } else {
      report.checks.sibling_interruption = {
        status: 'unsupported-native-host',
        reason: 'The deterministic actual-CLI failure uses a Windows file handle opened without FILE_SHARE_DELETE. POSIX rename can replace an open sibling, so this host cannot reproduce that interruption safely.',
        required_evidence: 'Run this rehearsal on native Windows and retain the locked-sibling first attempt plus successful retry transcript.',
      };
      report.status = 'partial';
      report.unfinished = 'Native Windows sibling replacement interruption and retry evidence is still required.';
      exitCode = 3;
    }

    if (report.status === 'running') {
      report.status = 'passed';
      exitCode = 0;
    }
  } catch (error) {
    report.status = 'failed';
    report.failure = error instanceof Error ? error.message : String(error);
    exitCode = 1;
  } finally {
    if (fixtureRoot) {
      try {
        const stat = fs.lstatSync(fixtureRoot);
        if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Private fixture root changed type before cleanup');
        fs.rmSync(fixtureRoot, { recursive: true, force: false });
        report.fixture_cleanup = 'removed';
      } catch (error) {
        report.fixture_cleanup = 'failed';
        report.fixture_cleanup_error = error instanceof Error ? error.message : String(error);
        report.status = 'failed';
        exitCode = 1;
      }
    } else {
      report.fixture_cleanup = 'not-created';
    }
    report.finished_at_utc = new Date().toISOString();
    try {
      const artifactSha256 = writeArtifact(output, report);
      process.stdout.write(`Rehearsal ${report.status}: ${path.relative(root, output.file)}\n`);
      process.stdout.write(`Artifact SHA-256: ${artifactSha256}\n`);
    } catch (error) {
      process.stderr.write(`Could not write rehearsal artifact: ${error instanceof Error ? error.message : String(error)}\n`);
      report.status = 'failed';
      exitCode = 1;
    }
  }
  return exitCode;
}

async function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n${usage()}\n`);
    return 2;
  }
  if (options.help) {
    process.stdout.write(`${usage()}\n`);
    return 0;
  }

  let target;
  let output;
  try {
    target = nativeTarget();
    output = artifactLocation(options.output, target);
    ensureArtifactDirectory(output.directory);
    try {
      fs.lstatSync(output.file);
      throw new UsageError(`Refusing to overwrite rehearsal artifact: ${path.relative(root, output.file)}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return error instanceof UsageError ? 2 : 1;
  }

  return runRehearsal(options, target, output);
}

if (require.main === module) {
  main()
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
      process.exitCode = 1;
    });
}
