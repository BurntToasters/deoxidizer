'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
// Use the same version and canonical URL rules as the version synchronizer.
const { cargoVersion, releaseAssetNames, validateChangelogReleaseLinks } = require('./sync-version.cjs');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const changelog = fs.readFileSync(path.join(root, 'CHANGELOG.md'), 'utf8');
// Shared [package]-scoped parser; do not use an inline ^version regex here
// (a dependency's version line could shadow the package version).
const version = cargoVersion(manifest);

const requiredMarkers = [
  '# ⬇️ Downloads',
  '> [!IMPORTANT]',
  `## Changes in \`v${version}:\``,
  '## ℹ️ Release Info',
  '### This changelog is made using the BCLS standard:',
];
const targets = releaseAssetNames(version);
const errors = [];

for (const marker of requiredMarkers) {
  if (!changelog.includes(marker)) errors.push(`missing changelog marker: ${marker}`);
}
for (const target of targets) {
  if (!changelog.includes(target)) errors.push(`missing download asset: ${target}`);
}
errors.push(...validateChangelogReleaseLinks(changelog, version));
if (/\b(?:TODO|TBD|FIXME)\b/i.test(changelog)) {
  errors.push('changelog contains an unfinished placeholder');
}

if (errors.length > 0) {
  console.error(errors.map((error) => `- ${error}`).join('\n'));
  process.exit(1);
}

console.log(`Changelog consistent with BCLS release format: ${version}`);
