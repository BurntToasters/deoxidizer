'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');

function parsePublicKey(value) {
  const text = String(value);
  const match = text.match(/^untrusted comment: minisign public key: ([0-9A-F]{16})\n([A-Za-z0-9+/]{56})\n$/);
  if (!match) throw new Error('release-signing-key.pub must be the canonical two-line Minisign key');
  const [, commentKeyId, encoded] = match;
  const bytes = Buffer.from(encoded, 'base64');
  if (bytes.length !== 42 || bytes.toString('base64') !== encoded || bytes.subarray(0, 2).toString('ascii') !== 'Ed') {
    throw new Error('release-signing-key.pub contains an unsupported or malformed key');
  }
  // Minisign serializes the key ID little-endian but displays it big-endian.
  const keyId = Buffer.from(bytes.subarray(2, 10)).reverse().toString('hex').toUpperCase();
  if (keyId !== commentKeyId) throw new Error('Minisign public key comment key ID does not match its serialized key');
  return { text, keyId, encoded, bytes };
}

function validateKeyPins(publicKeyText, files) {
  const key = parsePublicKey(publicKeyText);
  const runtime = String(files['src/minisign.rs'] || '');
  if (
    !/include_str!\(\s*"\.\.\/release-signing-key\.pub"\s*\)/.test(runtime) ||
    !runtime.includes(`untrusted comment: minisign public key: ${key.keyId}`) ||
    !runtime.includes(key.encoded)
  ) {
    throw new Error('src/minisign.rs does not pin the exact release-signing-key.pub serialization');
  }
  for (const name of ['install.sh', 'install.ps1']) {
    const source = String(files[name] || '');
    if (!source.includes(key.keyId) || !source.includes(key.encoded)) {
      throw new Error(`${name} does not pin the exact Minisign public key`);
    }
  }
  return { keyId: key.keyId };
}

function main() {
  const publicKey = fs.readFileSync(path.join(root, 'release-signing-key.pub'), 'utf8');
  const files = Object.fromEntries(
    ['src/minisign.rs', 'install.sh', 'install.ps1'].map((relative) => [
      relative,
      fs.readFileSync(path.join(root, relative), 'utf8'),
    ]),
  );
  const { keyId } = validateKeyPins(publicKey, files);
  console.log(`Release Minisign key consistent: ${keyId}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Release key check failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { parsePublicKey, validateKeyPins, main };
