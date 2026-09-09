'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const zlib = require('node:zlib');
const {
  assertGitHubCliAuthenticated,
  githubApi,
  repository,
  runGitHub,
} = require('./github-cli.cjs');
const { expectedReleaseAssets } = require('./verify-release-draft.cjs');

const root = path.resolve(__dirname, '..');
const releaseDir = path.join(root, 'release');
const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES = 512 * 1024 * 1024;
const MAX_REMOTE_MANIFEST_BYTES = 4 * 1024 * 1024;
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error('Cargo.toml has no version');
const tag = `v${version}`;

function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function parseChecksumManifest(text) {
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = line.match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (!match) throw new Error(`Invalid checksum line: ${line}`);
    if (entries.has(match[2])) throw new Error(`Duplicate checksum entry: ${match[2]}`);
    entries.set(match[2], match[1].toLowerCase());
  }
  return entries;
}

function verifyChecksum(filePath, expected) {
  return sha256(filePath).toLowerCase() === expected.toLowerCase();
}

function verifyDetachedSignature(filePath) {
  const signaturePath = `${filePath}.asc`;
  const keyWorkspace = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-release-key-'));
  const keyPath = path.join(keyWorkspace, 'release-signing-key.asc');
  const keyringPath = path.join(keyWorkspace, 'release-keyring.gpg');
  fs.copyFileSync(path.join(root, 'release-signing-key.asc'), keyPath);
  const dearmor = spawnSync(
    'gpg',
    ['--batch', '--yes', '--dearmor', '--output', keyringPath, keyPath],
    { encoding: 'utf8' },
  );
  if (dearmor.error || dearmor.status !== 0) {
    fs.rmSync(keyWorkspace, { recursive: true, force: true });
    throw new Error('Cannot load pinned release signing key');
  }
  const result = spawnSync(
    'gpg',
    [
      '--batch',
      '--no-options',
      '--no-default-keyring',
      '--keyring',
      keyringPath,
      '--status-fd',
      '1',
      '--verify',
      signaturePath,
      filePath,
    ],
    { encoding: 'utf8' },
  );
  fs.rmSync(keyWorkspace, { recursive: true, force: true });
  if (result.error || result.status !== 0 || !/^\[GNUPG:\] VALIDSIG /m.test(result.stdout || '')) {
    throw new Error(`Invalid detached signature: ${path.basename(signaturePath)}`);
  }
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
  if (!fs.existsSync(releaseDir)) return [];
  return fs
    .readdirSync(releaseDir)
    .filter((name) => !name.startsWith('.') && fs.statSync(path.join(releaseDir, name)).isFile())
    .map((name) => path.join(releaseDir, name));
}

function verifyLocal() {
  const archives = expectedArchives();
  if (archives.length === 0) throw new Error(`No release archives found in ${releaseDir}`);
  const checksumPaths = allReleaseFiles().filter((filePath) =>
    /^SHA256SUMS(?:-[a-z0-9_-]+)?\.txt$/i.test(path.basename(filePath)),
  );
  if (checksumPaths.length === 0) throw new Error('No SHA256SUMS*.txt manifest found');
  const entries = new Map();
  for (const checksumPath of checksumPaths) {
    for (const [name, digest] of parseChecksumManifest(fs.readFileSync(checksumPath, 'utf8'))) {
      if (entries.has(name) && entries.get(name) !== digest) {
        throw new Error(`Conflicting checksum entries for ${name}`);
      }
      entries.set(name, digest);
    }
  }

  const signedAssets = allReleaseFiles().filter(
    (filePath) =>
      !checksumPaths.some((checksumPath) => path.basename(checksumPath) === path.basename(filePath)) &&
      !path.basename(filePath).endsWith('.asc'),
  );
  const localNames = new Set(allReleaseFiles().map((filePath) => path.basename(filePath)));
  for (const name of entries.keys()) {
    if (!localNames.has(name)) throw new Error(`Checksum manifest lists missing asset: ${name}`);
  }
  for (const filePath of signedAssets) {
    const name = path.basename(filePath);
    const expected = entries.get(name);
    if (!expected || !verifyChecksum(filePath, expected)) {
      throw new Error(`Checksum mismatch or missing entry: ${name}`);
    }
    if (filePath.endsWith('.tar.gz') || filePath.endsWith('.zip')) {
      verifyArchiveIntegrity(filePath);
      const names = archiveEntries(filePath);
      const windows = /-windows-/.test(name);
      const expectedNames = windows
        ? new Set(['deoxidizer.exe', 'deox.exe', 'LICENSE'])
        : new Set(['deoxidizer', 'deox', 'LICENSE']);
      for (const required of expectedNames) {
        if (!names.has(required)) throw new Error(`${name} is missing ${required}`);
      }
      for (const entry of names) {
        if (!expectedNames.has(entry)) throw new Error(`${name} contains unexpected entry ${entry}`);
      }
    }
    if (
      process.env.DEOX_ALLOW_UNSIGNED_RELEASE !== '1' &&
      !fs.existsSync(`${filePath}.asc`)
    ) {
      throw new Error(`Missing detached signature: ${path.basename(filePath)}.asc`);
    }
    if (process.env.DEOX_ALLOW_UNSIGNED_RELEASE !== '1') verifyDetachedSignature(filePath);
  }
  if (process.env.DEOX_ALLOW_UNSIGNED_RELEASE !== '1') {
    for (const checksumPath of checksumPaths) {
      if (!fs.existsSync(`${checksumPath}.asc`)) {
        throw new Error(`Missing detached signature: ${path.basename(checksumPath)}.asc`);
      }
      verifyDetachedSignature(checksumPath);
    }
  }
  return archives;
}

function listRemoteAssets(releaseId) {
  const assets = [];
  for (let page = 1; ; page += 1) {
    const batch = githubApi(
      'GET',
      `/repos/${repository()}/releases/${releaseId}/assets?per_page=100&page=${page}`,
    );
    if (!Array.isArray(batch)) throw new Error('GitHub returned invalid asset list');
    assets.push(...batch);
    if (batch.length < 100) return assets;
  }
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

function downloadRemoteAsset(asset) {
  if (!Number.isInteger(asset?.id)) throw new Error(`Remote asset ${asset?.name || '<unknown>'} has no ID`);
  const result = runGitHub([
    'api',
    '-H',
    'Accept: application/octet-stream',
    `/repos/${repository()}/releases/assets/${asset.id}`,
  ]);
  const bytes = Buffer.from(result.stdout || '', 'utf8');
  if (bytes.length > MAX_REMOTE_MANIFEST_BYTES) {
    throw new Error(`Remote checksum asset exceeds size limit: ${asset.name}`);
  }
  const digest = `sha256:${crypto.createHash('sha256').update(bytes).digest('hex')}`;
  if (String(asset.digest || '').toLowerCase() !== digest) {
    throw new Error(`Remote digest changed while reading ${asset.name}`);
  }
  return bytes;
}

function validateRemoteManifestEntries(manifestName, manifestText, assets) {
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
    if (asset && String(asset.digest || '').toLowerCase() !== `sha256:${digest}`) {
      errors.push(`${manifestName} digest mismatch for ${name}`);
    }
  }
  return errors;
}

function verifyRemoteManifests(remoteAssets) {
  const assets = new Map(remoteAssets.map((asset) => [asset.name, asset]));
  const manifests = remoteAssets.filter((asset) =>
    /^SHA256SUMS-(?:linux|darwin|windows)-(?:x86_64|aarch64)\.txt$/i.test(asset.name),
  );
  if (manifests.length === 0) throw new Error('Remote release has no platform checksum manifest');

  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-remote-manifest-'));
  try {
    for (const manifestAsset of manifests) {
      const signatureAsset = assets.get(`${manifestAsset.name}.asc`);
      if (!signatureAsset) {
        throw new Error(`Remote release is missing ${manifestAsset.name}.asc`);
      }
      const manifestPath = path.join(workspace, manifestAsset.name);
      const signaturePath = `${manifestPath}.asc`;
      const manifestBytes = downloadRemoteAsset(manifestAsset);
      const signatureBytes = downloadRemoteAsset(signatureAsset);
      fs.writeFileSync(manifestPath, manifestBytes);
      fs.writeFileSync(signaturePath, signatureBytes);
      verifyDetachedSignature(manifestPath);
      const errors = validateRemoteManifestEntries(
        manifestAsset.name,
        manifestBytes.toString('utf8'),
        assets,
      );
      if (errors.length > 0) throw new Error(errors.join('\n'));
    }
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

function verifyRemote(localFiles) {
  assertGitHubCliAuthenticated();
  const releases = githubApi('GET', `/repos/${repository()}/releases?per_page=100`);
  const release = releases.find((item) => item?.tag_name === tag);
  if (!release) throw new Error(`Release ${tag} not found`);
  if (!release.draft) throw new Error(`Release ${tag} is published; refusing remote mutation check`);
  const commitResult = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  const commit = String(commitResult.stdout || '').trim();
  if (commitResult.error || commitResult.status !== 0 || !/^[0-9a-f]{40}$/i.test(commit)) {
    throw new Error('Cannot determine current Git commit for remote release verification');
  }
  if (release.target_commitish?.toLowerCase() !== commit.toLowerCase()) {
    throw new Error(`Remote release target commit does not match current HEAD ${commit}`);
  }
  const remoteAssets = listRemoteAssets(release.id);
  const nameErrors = validateRemoteAssetNames(remoteAssets);
  if (nameErrors.length > 0) throw new Error(nameErrors.join('\n'));
  const assets = new Map(remoteAssets.map((asset) => [asset.name, asset]));
  for (const filePath of localFiles) {
    const name = path.basename(filePath);
    const asset = assets.get(name);
    if (!asset) throw new Error(`Remote release missing ${name}`);
    if (asset.size !== fs.statSync(filePath).size) {
      throw new Error(`Remote size mismatch for ${name}`);
    }
    const expectedDigest = `sha256:${sha256(filePath)}`;
    if (asset.digest !== expectedDigest) {
      throw new Error(`Remote digest missing or mismatched for ${name}`);
    }
  }
  verifyRemoteManifests(remoteAssets);
}

function main() {
  const localFiles = verifyLocal();
  if (process.argv.includes('--remote')) {
    verifyRemote(allReleaseFiles());
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
  verifyRemote,
  validateRemoteAssetNames,
  validateRemoteManifestEntries,
  allReleaseFiles,
};
