'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { cargoVersion } = require('./sync-version.cjs');
const { buildEnvironment, minisignEnvironment } = require('./release.cjs');

const root = path.resolve(__dirname, '..');
const MAX_SIGNATURE_BYTES = 64 * 1024;
const MAX_TAURI_WRAPPER_BYTES = Math.ceil((MAX_SIGNATURE_BYTES * 4) / 3) + 16;

function signerInvocation(filePath) {
  return {
    command: 'cargo',
    args: ['tauri', 'signer', 'sign', filePath],
  };
}

function signaturePath(filePath) {
  return `${filePath}.minisig`;
}

function decodeTauriSignature(value) {
  if (Buffer.isBuffer(value) && value.length > MAX_TAURI_WRAPPER_BYTES) {
    throw new Error('Tauri signature wrapper exceeds the size limit');
  }
  const encoded = Buffer.isBuffer(value) ? value.toString('ascii').trim() : String(value).trim();
  if (encoded.length > MAX_TAURI_WRAPPER_BYTES) {
    throw new Error('Tauri signature wrapper exceeds the size limit');
  }
  if (!encoded || !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) {
    throw new Error('Tauri signature wrapper is not strict base64');
  }
  const bytes = Buffer.from(encoded, 'base64');
  if (!bytes.length || bytes.length > MAX_SIGNATURE_BYTES || bytes.toString('base64') !== encoded) {
    throw new Error('Tauri signature wrapper has invalid base64 data or size');
  }
  const text = bytes.toString('ascii');
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  if (lines.at(-1) === '') lines.pop();
  if (
    lines.length !== 4 ||
    !lines[0].startsWith('untrusted comment: ') ||
    !/^[A-Za-z0-9+/]+=*$/.test(lines[1]) ||
    !lines[2].startsWith('trusted comment: ') ||
    !/^[A-Za-z0-9+/]+=*$/.test(lines[3])
  ) {
    throw new Error('Tauri signature wrapper does not contain a four-line Minisign signature');
  }
  return bytes;
}

function sha256File(filePath) {
  const hash = crypto.createHash('sha256');
  const descriptor = fs.openSync(filePath, 'r');
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

function readVersion() {
  return cargoVersion(fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'));
}

function assetDetails(fileName, version) {
  const escapedVersion = version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = fileName.match(
    new RegExp(`^deoxidizer-v${escapedVersion}-(linux|darwin|windows)-(x86_64|aarch64)(?:\\.(tar\\.gz|zip)|-setup\\.exe)$`, 'i'),
  );
  if (!match) return null;
  return { os: match[1].toLowerCase(), arch: match[2].toLowerCase(), name: fileName };
}

function targetMapping({ os, arch }) {
  const checksums = `SHA256SUMS-${os}-${arch}.txt`;
  const target = {
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
  }[os]?.[arch];
  if (!target) throw new Error('Unsupported release target for Minisign signing');
  return { checksums, target };
}

function releasePayloads(directory, version) {
  const payloads = [];
  for (const name of fs.readdirSync(directory)) {
    if (name.startsWith('.')) continue;
    const filePath = path.join(directory, name);
    const stats = fs.lstatSync(filePath);
    if (directory === path.join(root, 'release') && name === 'staging' && stats.isDirectory()) continue;
    if (!stats.isFile() || stats.isSymbolicLink()) {
      throw new Error(`Release signing entry is not a regular file: ${name}`);
    }
    const detail = assetDetails(name, version);
    if (detail) payloads.push({ ...detail, filePath });
    else throw new Error(`Unexpected release signing input: ${name}`);
  }
  return payloads.sort((left, right) => left.name.localeCompare(right.name));
}

function verifyReleaseSignature(filePath, sidecarPath = signaturePath(filePath), environment = process.env) {
  const result = spawnSync(
    'cargo',
    ['run', '--quiet', '--locked', '--example', 'verify_release_signature', '--', filePath, sidecarPath],
    { cwd: root, env: buildEnvironment(environment), encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 },
  );
  if (result.error || result.status !== 0) {
    throw new Error(
      `Pinned Minisign verification failed for ${path.basename(filePath)}` +
        `${result.error ? `: ${result.error.message}` : ''}` +
        `${result.stderr ? `\n${String(result.stderr).trim()}` : ''}`,
    );
  }
}

function signFile(filePath, environment) {
  const wrapperPath = `${filePath}.sig`;
  const sidecarPath = signaturePath(filePath);
  for (const existing of [wrapperPath, sidecarPath]) {
    try {
      fs.lstatSync(existing);
      throw new Error(`Refusing to replace existing release signature: ${path.basename(existing)}`);
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
    }
  }
  const invocation = signerInvocation(filePath);
  const result = spawnSync(invocation.command, invocation.args, {
    cwd: root,
    env: environment,
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    try {
      if (fs.existsSync(wrapperPath)) fs.unlinkSync(wrapperPath);
    } catch {}
    throw new Error(
      `Tauri Minisign signing failed for ${path.basename(filePath)}` +
        `${result.error ? `: ${result.error.message}` : ''}` +
        `${result.stderr ? `\n${String(result.stderr).trim()}` : ''}`,
    );
  }
  try {
    const wrapperStats = fs.lstatSync(wrapperPath);
    if (!wrapperStats.isFile() || wrapperStats.isSymbolicLink()) {
      throw new Error('Tauri signer did not create a regular .sig wrapper');
    }
    if (wrapperStats.size > MAX_TAURI_WRAPPER_BYTES) {
      throw new Error('Tauri signer output exceeds the signature size limit');
    }
    const signature = decodeTauriSignature(fs.readFileSync(wrapperPath));
    fs.writeFileSync(sidecarPath, signature, { flag: 'wx', mode: 0o600 });
    verifyReleaseSignature(filePath, sidecarPath, environment);
  } catch (error) {
    try {
      if (fs.existsSync(sidecarPath)) fs.unlinkSync(sidecarPath);
    } catch {}
    throw error;
  } finally {
    try {
      if (fs.existsSync(wrapperPath)) fs.unlinkSync(wrapperPath);
    } catch {}
  }
  return sidecarPath;
}

function writeChecksumManifest(payloads, filePath) {
  const lines = payloads.map(({ filePath: payloadPath }) =>
    `${sha256File(payloadPath)}  ${path.basename(payloadPath)}`,
  );
  const descriptor = fs.openSync(filePath, 'wx', 0o600);
  try {
    fs.writeFileSync(descriptor, `${lines.join('\n')}\n`);
    fs.fsyncSync(descriptor);
  } catch (error) {
    try {
      fs.closeSync(descriptor);
    } catch {}
    try {
      fs.unlinkSync(filePath);
    } catch {}
    throw error;
  }
  fs.closeSync(descriptor);
}

function parseArguments(argv) {
  let directory;
  let allowUnsigned = false;
  for (const argument of argv) {
    if (argument === '--allow-unsigned') allowUnsigned = true;
    else if (argument.startsWith('--')) throw new Error(`Unknown option: ${argument}`);
    else if (!directory) directory = argument === 'release' || argument === '.' ? argument : argument;
    else throw new Error(`Unexpected argument: ${argument}`);
  }
  return { directory, allowUnsigned };
}

function main(argv = process.argv.slice(2), environment = process.env) {
  const args = parseArguments(argv);
  const directory = path.resolve(root, args.directory || environment.DEOX_RELEASE_STAGE_DIR || 'release');
  const directoryStats = fs.lstatSync(directory);
  if (!directoryStats.isDirectory() || directoryStats.isSymbolicLink()) {
    throw new Error('Release signing directory must be a real directory');
  }
  const version = readVersion();
  const payloads = releasePayloads(directory, version);
  const archives = payloads.filter((asset) => asset.name.endsWith('.tar.gz') || asset.name.endsWith('.zip'));
  if (archives.length !== 1) throw new Error('A target stage must contain exactly one release archive');
  const detail = { os: archives[0].os, arch: archives[0].arch };
  if (payloads.some((asset) => asset.os !== detail.os || asset.arch !== detail.arch)) {
    throw new Error('Release stage mixes assets from different targets');
  }
  const mapping = targetMapping(detail);
  if (environment.DEOX_RELEASE_TARGET && environment.DEOX_RELEASE_TARGET !== mapping.target) {
    throw new Error('Release target does not match staged payload names');
  }
  const checksumName = environment.DEOX_CHECKSUM_NAME || mapping.checksums;
  if (checksumName !== mapping.checksums) throw new Error('Checksum name does not match staged target');
  if (detail.os === 'windows' && payloads.length !== 2) {
    throw new Error('Windows release stage must contain its archive and setup installer');
  }
  if (detail.os !== 'windows' && payloads.length !== 1) {
    throw new Error('Non-Windows release stage must contain exactly one archive');
  }
  const manifestPath = path.join(directory, checksumName);
  try {
    fs.lstatSync(manifestPath);
    throw new Error(`Refusing to replace existing checksum manifest ${checksumName}`);
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
  writeChecksumManifest(payloads, manifestPath);

  const allowUnsigned = args.allowUnsigned || environment.DEOX_ALLOW_UNSIGNED_RELEASE === '1';
  if (allowUnsigned) {
    if (environment.DEOX_RELEASE_CONFIRM !== 'YES') {
      throw new Error('Unsigned staging requires DEOX_RELEASE_CONFIRM=YES explicitly');
    }
    console.log('Unsigned local staging enabled; no signature sidecars were generated.');
    return;
  }

  const privateKeyPath = String(environment.TAURI_SIGNING_PRIVATE_KEY_PATH || '').trim();
  if (!privateKeyPath) throw new Error('TAURI_SIGNING_PRIVATE_KEY_PATH is required on the provisioned signer');
  let keyStats;
  try {
    keyStats = fs.lstatSync(privateKeyPath);
  } catch {
    throw new Error('The provisioned Tauri signing key file is unavailable');
  }
  if (!keyStats.isFile() || keyStats.isSymbolicLink()) {
    throw new Error('The provisioned Tauri signing key must be a regular non-symlink file');
  }
  const signingEnv = minisignEnvironment(environment);
  for (const { filePath } of [...payloads, { filePath: manifestPath }]) {
    const sidecarPath = signFile(filePath, signingEnv);
    console.log(`signed ${path.basename(filePath)} -> ${path.basename(sidecarPath)}`);
  }
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Release Minisign signing failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = {
  decodeTauriSignature,
  signerInvocation,
  signaturePath,
  buildEnvironment,
  minisignEnvironment,
  sha256File,
  verifyReleaseSignature,
  signFile,
  main,
};
