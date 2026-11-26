'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const zlib = require('node:zlib');
const {
  assertGitHubCliAuthenticated,
  downloadGitHubAsset,
  findReleaseByTag,
  githubApiWithRetry,
  listReleaseAssets: listAssetsByRelease,
  repository,
} = require('./github-cli.cjs');
const {
  expectedReleaseAssets,
  resolveTagCommit,
  validateDraft,
} = require('./verify-release-draft.cjs');
const { buildEnvironment, signaturePath, verifyReleaseSignature } = require('./minisign-sign.cjs');
const { cargoVersion } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const releaseRoot = path.join(root, 'release');
const releaseDir = process.env.DEOX_RELEASE_STAGE_DIR
  ? path.resolve(process.env.DEOX_RELEASE_STAGE_DIR)
  : releaseRoot;
const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES = 512 * 1024 * 1024;
const MAX_REMOTE_MANIFEST_BYTES = 4 * 1024 * 1024;
const MAX_REMOTE_SIGNATURE_BYTES = 64 * 1024;
const MAX_REMOTE_ASSET_BYTES = MAX_ARCHIVE_BYTES;
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = cargoVersion(manifest);
const tag = `v${version}`;

function assertReleaseDirectory() {
  if (releaseDir !== releaseRoot) {
    const stagingRoot = path.join(releaseRoot, 'staging');
    const relative = path.relative(stagingRoot, releaseDir);
    const target = path.basename(releaseDir);
    const supportedTargets = new Set([
      'x86_64-unknown-linux-musl',
      'aarch64-unknown-linux-musl',
      'x86_64-apple-darwin',
      'aarch64-apple-darwin',
      'x86_64-pc-windows-msvc',
      'aarch64-pc-windows-msvc',
    ]);
    if (
      relative.startsWith(`..${path.sep}`) ||
      relative === '..' ||
      path.isAbsolute(relative) ||
      !supportedTargets.has(target) ||
      relative !== target ||
      (process.env.DEOX_RELEASE_TARGET && process.env.DEOX_RELEASE_TARGET !== target)
    ) {
      throw new Error('Release verification stage is not a supported isolated target directory');
    }
  }
  if (fs.existsSync(releaseDir)) {
    const stats = fs.lstatSync(releaseDir);
    if (!stats.isDirectory() || stats.isSymbolicLink()) {
      throw new Error(`Release verification directory is not a real directory: ${releaseDir}`);
    }
  }
}

function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function normalizeDigest(value) {
  const digest = String(value || '').toLowerCase().replace(/^sha256:/, '');
  return /^[0-9a-f]{64}$/.test(digest) ? digest : null;
}

function normalizedBinaryDigests(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be a binary digest map`);
  }
  const result = new Map();
  for (const [name, digestValue] of Object.entries(value)) {
    const basename = path.parse(name).name.toLowerCase();
    const digest = normalizeDigest(digestValue);
    if (!['deoxidizer', 'deox'].includes(basename) || !digest || result.has(basename)) {
      throw new Error(`${label} contains an invalid or ambiguous binary digest`);
    }
    result.set(basename, digest);
  }
  if (result.size !== 2 || !result.has('deoxidizer') || !result.has('deox')) {
    throw new Error(`${label} must contain exactly both release binaries`);
  }
  return result;
}

function validateNativeProof(proof, expected) {
  if (!proof || typeof proof !== 'object' || proof.schema_version !== 1) {
    throw new Error('Native release proof is missing or has an unsupported schema');
  }
  if (proof.target !== expected.target) throw new Error('Native proof target does not match the archive target');
  if (!/^[0-9a-f]{40}$/i.test(String(proof.commit || ''))) {
    throw new Error('Native proof has no valid candidate commit');
  }
  if (proof.commit.toLowerCase() !== String(expected.commit || '').toLowerCase()) {
    throw new Error('Native proof commit does not match the release tag or build session');
  }
  if (proof.platform !== expected.platform) throw new Error('Native proof platform does not match the archive target');

  const proofBinaries = normalizedBinaryDigests(proof.artifacts, 'Native proof artifacts');
  const extractedBinaries = normalizedBinaryDigests(expected.binaries, 'Extracted archive binaries');
  for (const [name, digest] of extractedBinaries) {
    if (proofBinaries.get(name) !== digest) {
      throw new Error(`Native proof digest does not match extracted ${name} binary`);
    }
  }

  const native = proof.native;
  if (!native || typeof native !== 'object') throw new Error('Native proof has no platform verification record');
  if (expected.platform === 'darwin') {
    if (native.mechanism !== 'codesign' || native.signature_verified !== true) {
      throw new Error('macOS native proof does not confirm a verified code signature');
    }
    if (native.timestamp_verified !== true) throw new Error('macOS native proof lacks a verified signature timestamp');
    if (typeof native.team_id !== 'string' || !native.team_id.trim()) {
      throw new Error('macOS native proof lacks the verified Team ID');
    }
    const notarization = native.notarization;
    if (notarization?.status !== 'Accepted' || !Array.isArray(notarization.submissions)) {
      throw new Error('macOS native proof lacks accepted notarization records');
    }
    const submissions = new Map();
    for (const submission of notarization.submissions) {
      if (
        !submission ||
        !['deoxidizer', 'deox'].includes(String(submission.asset || '').toLowerCase()) ||
        typeof submission.submission_id !== 'string' ||
        !submission.submission_id.trim() ||
        submissions.has(String(submission.asset).toLowerCase())
      ) {
        throw new Error('macOS native proof has an invalid notarization submission record');
      }
      submissions.set(String(submission.asset).toLowerCase(), submission.submission_id);
    }
    if (submissions.size !== 2) throw new Error('macOS native proof must notarize both release binaries');
  } else if (expected.platform === 'windows') {
    if (native.mechanism !== 'authenticode' || native.signature_verified !== true) {
      throw new Error('Windows native proof does not confirm a verified Authenticode signature');
    }
    if (native.timestamp_verified !== true) throw new Error('Windows native proof lacks an RFC3161 timestamp');
    if (typeof native.publisher !== 'string' || !native.publisher.trim()) {
      throw new Error('Windows native proof lacks the verified publisher');
    }
    if (typeof native.subject !== 'string' || !native.subject.trim()) {
      throw new Error('Windows native proof lacks the verified certificate subject');
    }
    const installer = proof.installer;
    const installerDigest = normalizeDigest(installer?.sha256);
    const actualInstaller = expected.associatedAssets?.get(installer?.name);
    if (
      !installer ||
      typeof installer.name !== 'string' ||
      !installer.name.endsWith('-setup.exe') ||
      !installerDigest ||
      normalizeDigest(actualInstaller) !== installerDigest
    ) {
      throw new Error('Windows native proof installer digest does not match the signed setup asset');
    }
  } else if (expected.platform === 'linux') {
    if (native.mechanism !== 'none' || native.status !== 'not-required') {
      throw new Error('Linux proof must explicitly mark native signing as not required');
    }
  } else {
    throw new Error(`Unsupported native proof platform: ${expected.platform}`);
  }
  return true;
}

function makeReleaseSnapshot(release, assets, tagCommit) {
  return {
    releaseId: release?.id,
    tag: release?.tag_name,
    targetCommitish: String(release?.target_commitish || '').toLowerCase(),
    draft: Boolean(release?.draft),
    prerelease: Boolean(release?.prerelease),
    tagCommit: String(tagCommit || '').toLowerCase(),
    assets: [...assets]
      .map((asset) => ({
        id: asset?.id,
        name: asset?.name,
        size: asset?.size,
        digest: String(asset?.digest || '').toLowerCase(),
        browserDownloadUrl: asset?.browser_download_url || '',
      }))
      .sort((left, right) => String(left.name).localeCompare(String(right.name))),
  };
}

function assertSameReleaseSnapshot(expected, actual) {
  if (JSON.stringify(expected) !== JSON.stringify(actual)) {
    throw new Error('Remote release ID, tag commit, or asset snapshot changed during verification');
  }
  return true;
}

function parseChecksumManifest(text) {
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = line.match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (!match) throw new Error(`Invalid checksum line: ${line}`);
    const name = match[2].trim();
    if (!name) throw new Error(`Invalid checksum line: ${line}`);
    if (entries.has(name)) throw new Error(`Duplicate checksum entry: ${name}`);
    entries.set(name, match[1].toLowerCase());
  }
  return entries;
}

function verifyChecksum(filePath, expected) {
  return sha256(filePath).toLowerCase() === expected.toLowerCase();
}

// Cross-check a SHA256SUMS manifest using the pinned verifier runtime uses.
function verifyInProcessManifest(manifestPath) {
  const result = spawnSync(
    'cargo',
    [
      'run',
      '--quiet',
      '--locked',
      '--example',
      'verify_release_manifest',
      '--',
      manifestPath,
      `${manifestPath}.minisig`,
    ],
    { cwd: root, env: buildEnvironment(process.env), encoding: 'utf8' },
  );
  if (result.error || result.status !== 0) {
    throw new Error(
      `In-process manifest verification failed: ${path.basename(manifestPath)}` +
        `${result.error ? `: ${result.error.message}` : ''}` +
        `${result.stderr ? `\n${String(result.stderr).trim()}` : ''}`,
    );
  }
}

function verifyInProcessArchive(archivePath) {
  const result = spawnSync(
    'cargo',
    [
      'run',
      '--quiet',
      '--locked',
      '--example',
      'verify_release_archive',
      '--',
      '--json',
      '--require-proof',
      archivePath,
    ],
    { cwd: root, env: buildEnvironment(process.env), encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 },
  );
  if (result.error || result.status !== 0) {
    throw new Error(
      `Production archive verification failed: ${path.basename(archivePath)}` +
        `${result.error ? `: ${result.error.message}` : ''}` +
        `${result.stderr ? `\n${String(result.stderr).trim()}` : ''}`,
    );
  }
  let report;
  try {
    report = JSON.parse(String(result.stdout || ''));
  } catch (error) {
    throw new Error(`Production archive verifier returned invalid JSON: ${error.message}`);
  }
  const proof = report?.proof_json ?? report?.proof;
  if (!proof || typeof proof !== 'object' || !report?.binaries) {
    throw new Error(`Production archive verifier returned no native proof: ${path.basename(archivePath)}`);
  }
  return { binaries: report.binaries, proof };
}

function zipEntries(filePath) {
  const bytes = fs.readFileSync(filePath);
  const endOfCentralDirectorySignature = 0x06054b50;
  const endOfCentralDirectorySize = 22;
  const minimumSearchOffset = Math.max(
    0,
    bytes.length - endOfCentralDirectorySize - 0xffff,
  );
  let endOfCentralDirectoryOffset = -1;

  for (
    let offset = bytes.length - endOfCentralDirectorySize;
    offset >= minimumSearchOffset;
    offset -= 1
  ) {
    if (
      offset >= 0 &&
      bytes.readUInt32LE(offset) === endOfCentralDirectorySignature &&
      offset + endOfCentralDirectorySize + bytes.readUInt16LE(offset + 20) === bytes.length
    ) {
      endOfCentralDirectoryOffset = offset;
      break;
    }
  }

  if (endOfCentralDirectoryOffset < 0) {
    throw new Error(`Cannot inspect ZIP archive ${path.basename(filePath)}`);
  }

  const diskNumber = bytes.readUInt16LE(endOfCentralDirectoryOffset + 4);
  const centralDirectoryDisk = bytes.readUInt16LE(endOfCentralDirectoryOffset + 6);
  const entriesOnDisk = bytes.readUInt16LE(endOfCentralDirectoryOffset + 8);
  const entries = bytes.readUInt16LE(endOfCentralDirectoryOffset + 10);
  const centralDirectorySize = bytes.readUInt32LE(endOfCentralDirectoryOffset + 12);
  const centralDirectoryOffset = bytes.readUInt32LE(endOfCentralDirectoryOffset + 16);

  if (
    diskNumber !== 0 ||
    centralDirectoryDisk !== 0 ||
    entriesOnDisk !== entries ||
    entries === 0xffff ||
    centralDirectorySize === 0xffffffff ||
    centralDirectoryOffset === 0xffffffff
  ) {
    throw new Error(`Unsupported ZIP archive ${path.basename(filePath)}`);
  }

  const centralDirectoryEnd = centralDirectoryOffset + centralDirectorySize;
  if (
    centralDirectoryOffset > bytes.length ||
    centralDirectoryEnd > bytes.length ||
    centralDirectoryEnd > endOfCentralDirectoryOffset
  ) {
    throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
  }

  const names = new Set();
  let offset = centralDirectoryOffset;
  for (let index = 0; index < entries; index += 1) {
    if (offset + 46 > centralDirectoryEnd || bytes.readUInt32LE(offset) !== 0x02014b50) {
      throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
    }

    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const entryEnd = offset + 46 + nameLength + extraLength + commentLength;
    if (entryEnd > centralDirectoryEnd) {
      throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
    }

    const name = bytes.subarray(offset + 46, offset + 46 + nameLength).toString('utf8');
    if (names.has(name)) throw new Error(`Duplicate ZIP entry ${name}`);
    names.add(name);
    offset = entryEnd;
  }

  if (offset !== centralDirectoryEnd) {
    throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
  }
  return names;
}

function archiveEntries(filePath) {
  if (filePath.toLowerCase().endsWith('.zip')) {
    return zipEntries(filePath);
  }
  const result = spawnSync('tar', ['-tzf', filePath], { encoding: 'utf8' });
  if (result.error || result.status !== 0) {
    throw new Error(`Cannot inspect archive ${path.basename(filePath)}`);
  }
  const entries = String(result.stdout).split(/\r?\n/).filter(Boolean);
  if (new Set(entries).size !== entries.length) {
    throw new Error(`Duplicate TAR entry in ${path.basename(filePath)}`);
  }
  return new Set(entries);
}

function verifyArchiveIntegrity(filePath) {
  if (fs.statSync(filePath).size > MAX_ARCHIVE_BYTES) {
    throw new Error(`Archive exceeds size limit ${path.basename(filePath)}`);
  }
  if (filePath.toLowerCase().endsWith('.zip')) {
    verifyZipIntegrity(filePath);
    return;
  }
  const command = 'tar';
  const args = ['-tzf', filePath];
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.error || result.status !== 0) {
    throw new Error(`Archive integrity check failed for ${path.basename(filePath)}`);
  }
}

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function verifyZipIntegrity(filePath) {
  const bytes = fs.readFileSync(filePath);
  if (bytes.length > MAX_ARCHIVE_BYTES) {
    throw new Error(`ZIP archive exceeds size limit ${path.basename(filePath)}`);
  }
  const eocd = 0x06054b50;
  let end = -1;
  for (let offset = bytes.length - 22; offset >= Math.max(0, bytes.length - 22 - 0xffff); offset -= 1) {
    if (offset >= 0 && bytes.readUInt32LE(offset) === eocd) {
      const commentLength = bytes.readUInt16LE(offset + 20);
      if (offset + 22 + commentLength === bytes.length) {
        end = offset;
        break;
      }
    }
  }
  if (end < 0) throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
  const count = bytes.readUInt16LE(end + 10);
  const centralOffset = bytes.readUInt32LE(end + 16);
  const centralSize = bytes.readUInt32LE(end + 12);
  if (count === 0xffff || centralOffset + centralSize > end) {
    throw new Error(`Unsupported ZIP archive ${path.basename(filePath)}`);
  }
  let offset = centralOffset;
  let totalUncompressed = 0;
  for (let index = 0; index < count; index += 1) {
    if (offset + 46 > end || bytes.readUInt32LE(offset) !== 0x02014b50) {
      throw new Error(`Malformed ZIP archive ${path.basename(filePath)}`);
    }
    const flags = bytes.readUInt16LE(offset + 8);
    const method = bytes.readUInt16LE(offset + 10);
    const expectedCrc = bytes.readUInt32LE(offset + 16);
    const compressedSize = bytes.readUInt32LE(offset + 20);
    const uncompressedSize = bytes.readUInt32LE(offset + 24);
    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const localOffset = bytes.readUInt32LE(offset + 42);
    const entryEnd = offset + 46 + nameLength + extraLength + commentLength;
    if (
      flags & 0x1 ||
      compressedSize === 0xffffffff ||
      uncompressedSize === 0xffffffff ||
      localOffset + 30 > centralOffset ||
      entryEnd > centralOffset + centralSize
    ) {
      throw new Error(`Unsupported or malformed ZIP entry in ${path.basename(filePath)}`);
    }
    if (bytes.readUInt32LE(localOffset) !== 0x04034b50) {
      throw new Error(`Malformed ZIP local header in ${path.basename(filePath)}`);
    }
    const localNameLength = bytes.readUInt16LE(localOffset + 26);
    const localExtraLength = bytes.readUInt16LE(localOffset + 28);
    const centralName = bytes.subarray(offset + 46, offset + 46 + nameLength);
    const localName = bytes.subarray(localOffset + 30, localOffset + 30 + localNameLength);
    if (!centralName.equals(localName)) {
      throw new Error(`ZIP entry name mismatch in ${path.basename(filePath)}`);
    }
    const dataStart = localOffset + 30 + localNameLength + localExtraLength;
    const dataEnd = dataStart + compressedSize;
    if (dataStart > centralOffset || dataEnd > centralOffset || dataStart > dataEnd) {
      throw new Error(`ZIP entry exceeds archive bounds in ${path.basename(filePath)}`);
    }
    const compressed = bytes.subarray(dataStart, dataEnd);
    totalUncompressed += uncompressedSize;
    if (totalUncompressed > MAX_EXTRACTED_BYTES) {
      throw new Error(`ZIP archive exceeds extracted size limit ${path.basename(filePath)}`);
    }
    let uncompressed;
    try {
      uncompressed = method === 0 ? compressed : method === 8 ? zlib.inflateRawSync(compressed) : null;
    } catch {
      uncompressed = null;
    }
    if (!uncompressed || uncompressed.length !== uncompressedSize || crc32(uncompressed) !== expectedCrc) {
      throw new Error(`ZIP CRC or size mismatch in ${path.basename(filePath)}`);
    }
    offset = entryEnd;
  }
  if (offset !== centralOffset + centralSize) {
    throw new Error(`Malformed ZIP central directory ${path.basename(filePath)}`);
  }
}

function expectedArchives() {
  assertReleaseDirectory();
  if (!fs.existsSync(releaseDir)) return [];
  return fs
    .readdirSync(releaseDir)
    .filter(
      (name) =>
        name.startsWith(`deoxidizer-v${version}-`) &&
        (name.endsWith('.tar.gz') || name.endsWith('.zip')),
    )
    .map((name) => path.join(releaseDir, name));
}

function allReleaseFiles() {
  assertReleaseDirectory();
  if (!fs.existsSync(releaseDir)) return [];
  return fs
    .readdirSync(releaseDir)
    .filter((name) => !name.startsWith('.'))
    .flatMap((name) => {
      const filePath = path.join(releaseDir, name);
      const stats = fs.lstatSync(filePath);
      if (releaseDir === releaseRoot && name === 'staging' && stats.isDirectory()) return [];
      if (!stats.isFile() || stats.isSymbolicLink()) {
        throw new Error(`Staged release entry is not a regular file: ${name}`);
      }
      return [filePath];
    });
}

function targetForAsset(name) {
  const pattern = new RegExp(
    `^deoxidizer-v${version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}-(linux|darwin|windows)-(x86_64|aarch64)(?:\\.(?:tar\\.gz|zip)|-setup\\.exe)$`,
    'i',
  );
  const match = name.match(pattern);
  if (!match) return null;
  const [, osName, arch] = match;
  const os = osName.toLowerCase();
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
  }[os][arch];
  return { os, arch, target, platform: os };
}

function candidateCommit() {
  const commit = String(process.env.DEOX_RELEASE_COMMIT || '').trim();
  if (/^[0-9a-f]{40}$/i.test(commit)) return commit.toLowerCase();
  const result = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  const head = String(result.stdout || '').trim();
  if (result.error || result.status !== 0 || !/^[0-9a-f]{40}$/i.test(head)) {
    throw new Error('Cannot determine candidate commit for native archive proof');
  }
  return head.toLowerCase();
}

function verifyLocal() {
  if (
    process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1' &&
    process.env.DEOX_RELEASE_CONFIRM !== 'YES'
  ) {
    throw new Error('DEOX_ALLOW_UNSIGNED_RELEASE=1 requires DEOX_RELEASE_CONFIRM=YES explicitly');
  }
  const files = allReleaseFiles();
  const archives = expectedArchives();
  if (archives.length === 0) throw new Error(`No release archives found in ${releaseDir}`);
  const checksumPaths = files.filter((filePath) =>
    /^SHA256SUMS-(?:linux|darwin|windows)-(?:x86_64|aarch64)\.txt$/i.test(path.basename(filePath)),
  );
  if (checksumPaths.length === 0) throw new Error('No SHA256SUMS*.txt manifest found');
  const entries = new Map();
  const localAssets = new Map(files.map((filePath) => [path.basename(filePath), filePath]));
  const manifestTargets = new Set();
  const availableDigests = new Map(
    files.filter((filePath) => !path.basename(filePath).endsWith('.minisig'))
      .map((filePath) => [path.basename(filePath), sha256(filePath)]),
  );
  for (const checksumPath of checksumPaths) {
    const manifestName = path.basename(checksumPath);
    const manifestMatch = manifestName.match(/^SHA256SUMS-(linux|darwin|windows)-(x86_64|aarch64)\.txt$/i);
    const manifestTarget = `${manifestMatch[1].toLowerCase()}-${manifestMatch[2].toLowerCase()}`;
    if (manifestTargets.has(manifestTarget)) throw new Error(`Duplicate checksum manifest: ${manifestName}`);
    manifestTargets.add(manifestTarget);
    const manifestEntries = parseChecksumManifest(fs.readFileSync(checksumPath, 'utf8'));
    const assetMetadata = new Map(
      [...localAssets].map(([name]) => [name, { name, digest: availableDigests.has(name) ? `sha256:${availableDigests.get(name)}` : undefined }]),
    );
    const manifestErrors = validateRemoteManifestEntries(
      manifestName,
      fs.readFileSync(checksumPath, 'utf8'),
      assetMetadata,
      availableDigests,
    );
    if (manifestErrors.length > 0) throw new Error(manifestErrors.join('\n'));
    for (const [name, digest] of manifestEntries) {
      if (entries.has(name) && entries.get(name) !== digest) {
        throw new Error(`Conflicting checksum entries for ${name}`);
      }
      entries.set(name, digest);
    }
  }
  const nameErrors = validateRemoteAssetNames(files.map((filePath) => ({ name: path.basename(filePath) })));
  if (nameErrors.length > 0) throw new Error(nameErrors.join('\n'));
  const unsigned = process.env.DEOX_ALLOW_UNSIGNED_RELEASE === '1';
  const pairErrors = validateRemoteSignaturePairs(
    files.map((filePath) => ({ name: path.basename(filePath) })),
    { requireAll: !unsigned },
  );
  if (pairErrors.length > 0) throw new Error(pairErrors.join('\n'));
  const localNames = new Set(files.map((filePath) => path.basename(filePath)));
  for (const name of entries.keys()) {
    if (!localNames.has(name)) throw new Error(`Checksum manifest lists missing asset: ${name}`);
  }
  const payloads = files.filter((filePath) => !path.basename(filePath).endsWith('.minisig'));
  for (const filePath of payloads) {
    const name = path.basename(filePath);
    const descriptor = targetForAsset(name);
    const isManifest = checksumPaths.includes(filePath);
    if (!descriptor && !isManifest) throw new Error(`Unexpected release payload ${name}`);
    if (descriptor && releaseDir !== releaseRoot && descriptor.target !== path.basename(releaseDir)) {
      throw new Error(`Asset ${name} does not match isolated target ${path.basename(releaseDir)}`);
    }
    const expected = entries.get(name);
    if (!expected || !verifyChecksum(filePath, expected)) {
      throw new Error(`Checksum mismatch or missing entry: ${name}`);
    }
    const sidecar = signaturePath(filePath);
    if (localNames.has(path.basename(sidecar))) {
      verifyReleaseSignature(filePath, sidecar);
    } else if (!unsigned) {
      throw new Error(`Missing detached signature: ${path.basename(sidecar)}`);
    }
    if (isManifest && localNames.has(path.basename(sidecar))) verifyInProcessManifest(filePath);
    if (name.endsWith('.tar.gz') || name.endsWith('.zip')) {
      const archiveReport = verifyInProcessArchive(filePath);
      const associatedAssets = new Map();
      for (const [assetName] of entries) {
        const assetPath = localAssets.get(assetName);
        if (assetPath) associatedAssets.set(assetName, sha256(assetPath));
      }
      validateNativeProof(archiveReport.proof, {
        target: descriptor.target,
        commit: candidateCommit(),
        platform: descriptor.platform,
        binaries: archiveReport.binaries,
        associatedAssets,
      });
    }
  }
  return archives;
}

function validateRemoteAssetNames(assets) {
  const expected = expectedReleaseAssets();
  const names = new Set();
  const errors = [];
  for (const asset of assets) {
    if (!asset?.name) {
      errors.push('remote release contains an unnamed asset');
      continue;
    }
    if (names.has(asset.name)) errors.push(`remote release contains duplicate asset ${asset.name}`);
    names.add(asset.name);
    if (!expected.has(asset.name)) {
      errors.push(`remote release contains unexpected asset ${asset.name}`);
    }
  }
  return errors;
}

function validateRemoteSignaturePairs(assets, { requireAll = true } = {}) {
  const names = new Set(assets.map((asset) => asset?.name).filter(Boolean));
  const errors = [];
  for (const name of names) {
    if (name.endsWith('.minisig')) {
      const payload = name.slice(0, -'.minisig'.length);
      if (!names.has(payload)) errors.push(`remote release has orphan signature ${name}`);
      continue;
    }
    const signature = `${name}.minisig`;
    if (requireAll && !names.has(signature)) {
      errors.push(`remote release is missing detached signature ${signature}`);
    }
  }
  return errors;
}

function validateRemoteAssetDigests(assets, downloadedDigests) {
  const errors = [];
  for (const asset of assets) {
    const name = asset?.name || '<unknown>';
    const computed = normalizeDigest(downloadedDigests.get(name));
    if (!computed) {
      errors.push(`remote asset ${name} has no streamed SHA-256 hash`);
      continue;
    }
    if (!Number.isInteger(asset?.size) || asset.size <= 0) {
      errors.push(`remote asset ${name} has invalid size metadata`);
    }
    if (asset.digest !== undefined && asset.digest !== null && asset.digest !== '') {
      const metadata = normalizeDigest(asset.digest);
      if (!metadata) errors.push(`remote asset ${name} has invalid SHA-256 digest metadata`);
      else if (metadata !== computed) errors.push(`remote asset ${name} digest does not match streamed bytes`);
    }
  }
  return errors;
}

function expectedManifestEntries(name) {
  const match = name.match(/^SHA256SUMS-(linux|darwin|windows)-(x86_64|aarch64)\.txt$/i);
  if (!match) throw new Error(`Unsupported remote checksum manifest ${name}`);
  const [, os, arch] = match;
  const extension = os.toLowerCase() === 'windows' ? 'zip' : 'tar.gz';
  const archive = `deoxidizer-v${version}-${os.toLowerCase()}-${arch}.${extension}`;
  const expected = [archive];
  if (os.toLowerCase() === 'windows') {
    expected.push(`deoxidizer-v${version}-windows-${arch}-setup.exe`);
  }
  return new Set(expected);
}

function validateRemoteManifestEntries(manifestName, manifestText, assets, downloadedDigests = new Map()) {
  const entries = parseChecksumManifest(manifestText);
  const expected = expectedManifestEntries(manifestName);
  const errors = [];
  for (const name of expected) {
    if (!entries.has(name)) errors.push(`${manifestName} is missing ${name}`);
  }
  for (const name of entries.keys()) {
    if (!expected.has(name)) errors.push(`${manifestName} contains unexpected entry ${name}`);
    if (!assets.has(name)) errors.push(`${manifestName} lists missing remote asset ${name}`);
  }
  for (const [name, digest] of entries) {
    const asset = assets.get(name);
    if (!asset) continue;
    const streamed = normalizeDigest(downloadedDigests.get(name));
    const metadata = normalizeDigest(asset.digest);
    const actual = streamed || metadata;
    if (asset.digest && !metadata) {
      errors.push(`${manifestName} has invalid SHA-256 metadata for ${name}`);
    } else if (!actual) {
      errors.push(`${manifestName} has no verified SHA-256 hash for ${name}`);
    } else if (actual !== digest) {
      errors.push(`${manifestName} digest mismatch for ${name}`);
    }
  }
  return errors;
}

function remoteAssetLimit(name) {
  if (name.endsWith('.minisig')) return MAX_REMOTE_SIGNATURE_BYTES;
  if (/^SHA256SUMS-/.test(name)) return MAX_REMOTE_MANIFEST_BYTES;
  return MAX_REMOTE_ASSET_BYTES;
}

function downloadRemoteAssets(remoteAssets, workspace) {
  const localAssets = new Map();
  const downloadedDigests = new Map();
  for (const asset of remoteAssets) {
    if (!Number.isInteger(asset?.id) || asset.id <= 0) {
      throw new Error(`Remote asset ${asset?.name || '<unknown>'} has no valid ID`);
    }
    const filePath = path.join(workspace, asset.name);
    downloadGitHubAsset(asset.id, filePath, { maxBytes: remoteAssetLimit(asset.name) });
    const stats = fs.statSync(filePath);
    if (!Number.isInteger(asset.size) || asset.size !== stats.size) {
      throw new Error(`Remote size mismatch for ${asset.name}`);
    }
    localAssets.set(asset.name, filePath);
    downloadedDigests.set(asset.name, sha256(filePath));
  }
  const digestErrors = validateRemoteAssetDigests(remoteAssets, downloadedDigests);
  if (digestErrors.length > 0) throw new Error(digestErrors.join('\n'));
  return { localAssets, downloadedDigests };
}

function gitHeadCommit() {
  const result = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  const commit = String(result.stdout || '').trim().toLowerCase();
  if (result.error || result.status !== 0 || !/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error('Cannot determine current Git commit for remote release verification');
  }
  return commit;
}

function makeRemoteVerification(release, assets, tagCommit, downloadedDigests, commit) {
  const snapshotAssets = assets.map((asset) => ({
    ...asset,
    digest: `sha256:${normalizeDigest(downloadedDigests.get(asset.name))}`,
  }));
  return {
    release,
    commit,
    tagCommit,
    snapshot: makeReleaseSnapshot(release, snapshotAssets, tagCommit),
    assets: snapshotAssets,
  };
}

function verifyRemote(localFiles = [], { requireComplete = false } = {}) {
  assertGitHubCliAuthenticated();
  const release = findReleaseByTag(tag);
  if (!release) throw new Error(`Release ${tag} not found`);
  if (!release.draft) throw new Error(`Release ${tag} is published; refusing remote mutation check`);
  if (release.tag_name !== tag) throw new Error(`Remote release tag does not match ${tag}`);
  const commit = gitHeadCommit();
  if (release.target_commitish?.toLowerCase() !== commit.toLowerCase()) {
    throw new Error(`Remote release target commit does not match current HEAD ${commit}`);
  }
  if (process.env.DEOX_RELEASE_COMMIT && process.env.DEOX_RELEASE_COMMIT.toLowerCase() !== commit) {
    throw new Error('Build session commit does not match current Git HEAD');
  }
  const tagCommit = resolveTagCommit(tag);
  if (tagCommit.toLowerCase() !== commit) {
    throw new Error(`Git tag ${tag} resolves to ${tagCommit}, not current HEAD ${commit}`);
  }
  const remoteAssets = listAssetsByRelease(release.id);
  const nameErrors = validateRemoteAssetNames(remoteAssets);
  if (nameErrors.length > 0) throw new Error(nameErrors.join('\n'));
  if (remoteAssets.length === 0) throw new Error('Remote release has no assets to verify');
  const pairErrors = validateRemoteSignaturePairs(remoteAssets);
  if (pairErrors.length > 0) throw new Error(pairErrors.join('\n'));
  if (requireComplete) {
    const errors = validateDraft(release, remoteAssets, commit, tagCommit);
    if (errors.length > 0) throw new Error(errors.join('\n'));
  }
  const assets = new Map(remoteAssets.map((asset) => [asset.name, asset]));
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-remote-release-'));
  try {
    const { localAssets, downloadedDigests } = downloadRemoteAssets(remoteAssets, workspace);
    const payloads = remoteAssets.filter((asset) => !asset.name.endsWith('.minisig'));
    const manifests = payloads.filter((asset) => /^SHA256SUMS-/.test(asset.name));
    if (manifests.length === 0) throw new Error('Remote release has no platform checksum manifest');
    const allManifestEntries = new Map();
    const coveredAssets = new Set();
    for (const manifestAsset of manifests) {
      const manifestPath = localAssets.get(manifestAsset.name);
      const sidecarPath = `${manifestPath}.minisig`;
      verifyReleaseSignature(manifestPath, sidecarPath);
      verifyInProcessManifest(manifestPath);
      const errors = validateRemoteManifestEntries(
        manifestAsset.name,
        fs.readFileSync(manifestPath, 'utf8'),
        assets,
        downloadedDigests,
      );
      if (errors.length > 0) throw new Error(errors.join('\n'));
      for (const [name, digest] of parseChecksumManifest(fs.readFileSync(manifestPath, 'utf8'))) {
        if (allManifestEntries.has(name) && allManifestEntries.get(name) !== digest) {
          throw new Error(`Conflicting remote manifest entries for ${name}`);
        }
        allManifestEntries.set(name, digest);
        coveredAssets.add(name);
      }
    }
    const associatedAssets = new Map(
      [...downloadedDigests].map(([name, digest]) => [name, digest]),
    );
    for (const payload of payloads) {
      const name = payload.name;
      const filePath = localAssets.get(name);
      if (!/^SHA256SUMS-/.test(name) && !coveredAssets.has(name)) {
        throw new Error(`No signed checksum manifest covers remote payload ${name}`);
      }
      if (/^SHA256SUMS-/.test(name)) continue;
      verifyReleaseSignature(filePath, `${filePath}.minisig`);
      if (name.endsWith('.tar.gz') || name.endsWith('.zip')) {
        const descriptor = targetForAsset(name);
        if (!descriptor) throw new Error(`Unsupported remote archive ${name}`);
        const archiveReport = verifyInProcessArchive(filePath);
        validateNativeProof(archiveReport.proof, {
          target: descriptor.target,
          commit,
          platform: descriptor.platform,
          binaries: archiveReport.binaries,
          associatedAssets,
        });
      }
    }
    for (const localPath of localFiles) {
      const name = path.basename(localPath);
      const remoteDigest = downloadedDigests.get(name);
      if (!remoteDigest) throw new Error(`Remote release missing local staged asset ${name}`);
      if (sha256(localPath) !== remoteDigest) {
        throw new Error(`Remote bytes differ from staged ${name}`);
      }
    }
    return makeRemoteVerification(release, remoteAssets, tagCommit, downloadedDigests, commit);
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

function assertRemoteSnapshotUnchanged(verification) {
  if (!verification?.release?.id || !verification.snapshot) {
    throw new Error('A remote verification snapshot is required');
  }
  const release = githubApiWithRetry(
    'GET',
    `/repos/${repository()}/releases/${verification.release.id}`,
  );
  if (!release?.draft || release.id !== verification.release.id || release.tag_name !== tag) {
    throw new Error('Verified draft release identity changed before mutation');
  }
  const tagCommit = resolveTagCommit(tag);
  const assets = listAssetsByRelease(release.id);
  const names = validateRemoteAssetNames(assets);
  if (names.length > 0) throw new Error(names.join('\n'));
  let downloadedDigests = new Map();
  if (assets.some((asset) => !normalizeDigest(asset.digest))) {
    const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-remote-snapshot-'));
    try {
      ({ downloadedDigests } = downloadRemoteAssets(assets, workspace));
    } finally {
      fs.rmSync(workspace, { recursive: true, force: true });
    }
  } else {
    downloadedDigests = new Map(assets.map((asset) => [asset.name, normalizeDigest(asset.digest)]));
  }
  const actual = makeReleaseSnapshot(
    release,
    assets.map((asset) => ({
      ...asset,
      digest: `sha256:${downloadedDigests.get(asset.name)}`,
    })),
    tagCommit,
  );
  return assertSameReleaseSnapshot(verification.snapshot, actual);
}

function main() {
  if (process.argv.includes('--remote-complete')) {
    const verification = verifyRemote([], { requireComplete: true });
    assertRemoteSnapshotUnchanged(verification);
    console.log(`Release ${tag} passed complete remote integrity verification`);
    return;
  }
  const localFiles = verifyLocal();
  if (process.argv.includes('--remote')) {
    const verification = verifyRemote(allReleaseFiles());
    assertRemoteSnapshotUnchanged(verification);
  }
  console.log(`Release ${tag} verified (${localFiles.length} archive(s))`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ Release verification failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = {
  archiveEntries,
  verifyZipIntegrity,
  parseChecksumManifest,
  verifyChecksum,
  validateNativeProof,
  signaturePath,
  makeReleaseSnapshot,
  assertSameReleaseSnapshot,
  verifyRemote,
  validateRemoteAssetNames,
  validateRemoteManifestEntries,
  validateRemoteAssetDigests,
  validateRemoteSignaturePairs,
  assertRemoteSnapshotUnchanged,
  allReleaseFiles,
};
