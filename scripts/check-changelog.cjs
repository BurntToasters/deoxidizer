'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const changelog = fs.readFileSync(path.join(root, 'CHANGELOG.md'), 'utf8');
const version = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

if (!version) throw new Error('Cargo.toml has no package version');

const requiredMarkers = [
  '# ⬇️ Downloads',
  '> [!IMPORTANT]',
  `## Changes in \`v${version}:\``,
  '## ℹ️ Release Info',
  '### This changelog is made using the BCLS standard:',
];
const targets = [
  `deoxidizer-v${version}-windows-x86_64.zip`,
  `deoxidizer-v${version}-windows-aarch64.zip`,
  `deoxidizer-v${version}-windows-x86_64-setup.exe`,
  `deoxidizer-v${version}-windows-aarch64-setup.exe`,
  `deoxidizer-v${version}-darwin-x86_64.tar.gz`,
  `deoxidizer-v${version}-darwin-aarch64.tar.gz`,
  `deoxidizer-v${version}-linux-x86_64.tar.gz`,
  `deoxidizer-v${version}-linux-aarch64.tar.gz`,
];
const errors = [];

for (const marker of requiredMarkers) {
  if (!changelog.includes(marker)) errors.push(`missing changelog marker: ${marker}`);
}
for (const target of targets) {
  if (!changelog.includes(target)) errors.push(`missing download asset: ${target}`);
}
if (!changelog.includes(`/releases/download/v${version}/`)) {
  errors.push(`download links must point to v${version}`);
}
if (/\b(?:TODO|TBD|FIXME)\b/i.test(changelog)) {
  errors.push('changelog contains an unfinished placeholder');
}

if (errors.length > 0) {
  console.error(errors.map((error) => `- ${error}`).join('\n'));
  process.exit(1);
}

console.log(`Changelog consistent with BCLS release format: ${version}`);
