'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { assertGitHubCliAuthenticated, githubApi, repository } = require('./github-cli.cjs');
const { tag, verifyDraft } = require('./verify-release-draft.cjs');
const { verifyRemote, allReleaseFiles } = require('./verify-release.cjs');
const { cargoVersion, isPrerelease } = require('./sync-version.cjs');

const root = path.resolve(__dirname, '..');
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const version = cargoVersion(manifest);

function main() {
  if (process.argv[2] !== '--yes') {
    throw new Error('Publishing is irreversible; pass --yes after release:verify:draft');
  }
  if (process.env.DEOX_RELEASE_CONFIRM !== 'YES') {
    throw new Error('Publishing is irreversible; DEOX_RELEASE_CONFIRM=YES is required in addition to --yes');
  }
  assertGitHubCliAuthenticated();
  const release = verifyDraft();
  verifyRemote(allReleaseFiles());
  githubApi('PATCH', `/repos/${repository()}/releases/${release.id}`, {
    draft: false,
    prerelease: isPrerelease(version),
  });
  console.log(`Published ${tag} from draft release`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`✗ Publish failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}
