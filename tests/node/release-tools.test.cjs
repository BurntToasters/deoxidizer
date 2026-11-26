'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const { execFileSync, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const packageManifest = require('../../package.json');

const {
  normalizeArch,
  normalizeOs,
  TARGETS,
  buildEnvironment,
  verificationEnvironment,
  windowsFileVersion,
  releaseStageDirectory,
  npmReleaseInvocation,
  acquireStageLock,
  withDraftUploadLock,
} = require('../../scripts/release.cjs');
const {
  githubCliEnvironment,
  releaseMutationLockRef,
  validateExistingReleaseAsset,
} = require('../../scripts/github-cli.cjs');
const {
  archiveEntries,
  verifyZipIntegrity,
  parseChecksumManifest,
  verifyChecksum,
  validateRemoteAssetNames,
  validateRemoteManifestEntries,
  validateRemoteAssetDigests,
  validateRemoteSignaturePairs,
  validateNativeProof,
  signaturePath,
  makeReleaseSnapshot,
  assertSameReleaseSnapshot,
} = require('../../scripts/verify-release.cjs');
const { validateFresh, validateIdentity } = require('../../scripts/release-session.cjs');
const { requireConfirmation, parseArgs } = require('../../scripts/branch-sync.cjs');
const { requireConfirmation: requireViConfirmation } = require('../../scripts/vi.cjs');
const {
  requireConfirmation: requirePruneConfirmation,
} = require('../../scripts/git-prune.cjs');
const { parsePublicKey, validateKeyPins } = require('../../scripts/check-release-key.cjs');
const {
  decodeTauriSignature,
  signerInvocation,
  minisignEnvironment,
} = require('../../scripts/minisign-sign.cjs');
const { isPrerelease } = require('../../scripts/sync-version.cjs');
const {
  expectedReleaseAssets,
  validateDraft,
  peelTagCommit,
  tag,
  prerelease,
} = require('../../scripts/verify-release-draft.cjs');
const { releaseNotes } = require('../../scripts/ensure-draft-release.cjs');

function fakeGitHub({ release = null, assets = [], tagCommit, mutateOnLock } = {}) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-fake-github-'));
  const bin = path.join(directory, 'bin');
  fs.mkdirSync(bin);
  const statePath = path.join(directory, 'state.json');
  const current = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
  }).trim();
  fs.writeFileSync(
    statePath,
    JSON.stringify({
      release,
      assets,
      tagCommit: tagCommit || current,
      lock: null,
      mutateOnLock: mutateOnLock || null,
      events: [],
    }),
  );
  const fakeGhPath = path.join(bin, 'gh');
  fs.writeFileSync(
    fakeGhPath,
    `#!/usr/bin/env node
'use strict';
const fs = require('node:fs');
const statePath = process.env.DEOX_FAKE_GH_STATE;
const state = JSON.parse(fs.readFileSync(statePath, 'utf8'));
const args = process.argv.slice(2);
function save() { fs.writeFileSync(statePath, JSON.stringify(state)); }
function respond(value) { process.stdout.write(JSON.stringify(value)); }
if (args[0] === 'auth') process.exit(0);
if (args[0] !== 'api') { process.stderr.write('unexpected fake gh command: ' + args.join(' ') + '\\n'); process.exit(2); }
const method = args[args.indexOf('--method') + 1] || 'GET';
const endpoint = args.find((arg) => arg.startsWith('/repos/'));
const input = args.includes('--input') ? JSON.parse(fs.readFileSync(0, 'utf8')) : {};
state.events.push({ method, endpoint, lock: state.lock });
save();
if (method === 'POST' && endpoint.endsWith('/git/refs')) {
  state.lock = input.sha;
  if (state.mutateOnLock) {
    if (state.mutateOnLock === 'absent') state.release = null;
    else if (state.release && state.mutateOnLock === 'id') state.release.id += 1;
    else if (state.release && state.mutateOnLock === 'published') state.release.draft = false;
    else if (state.release && state.mutateOnLock === 'target') state.release.target_commitish = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
    else if (state.mutateOnLock === 'tag') state.tagCommit = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
    state.mutateOnLock = null;
  }
  save(); respond({ ref: input.ref, object: { sha: input.sha } }); process.exit(0);
}
if (method === 'POST' && endpoint.endsWith('/releases')) {
  state.release = { ...input, id: 73, assets: state.assets };
  save(); respond(state.release); process.exit(0);
}
if (method === 'PATCH' && /\\/releases\\/\\d+$/.test(endpoint)) {
  state.release = { ...state.release, ...input };
  save(); respond(state.release); process.exit(0);
}
if (method === 'GET' && endpoint.endsWith('/releases?per_page=100&page=1')) {
  respond(state.release ? [state.release] : []); process.exit(0);
}
if (method === 'GET' && /\\/releases\\/tags\\//.test(endpoint)) {
  respond(state.release || {}); process.exit(0);
}
if (method === 'GET' && /\\/releases\\/\\d+\\/assets\\?/.test(endpoint)) {
  respond(state.assets); process.exit(0);
}
if (method === 'GET' && /\\/releases\\/\\d+$/.test(endpoint)) {
  respond(state.release || {}); process.exit(0);
}
if (method === 'GET' && /\\/git\\/ref\\/tags\\//.test(endpoint)) {
  respond({ object: { type: 'commit', sha: state.tagCommit } }); process.exit(0);
}
if (method === 'GET' && /\\/git\\/ref\\/heads\\/deoxidizer-release-lock-/.test(endpoint)) {
  respond({ object: { sha: state.lock } }); process.exit(0);
}
if (method === 'DELETE' && /\\/git\\/heads\\/deoxidizer-release-lock-/.test(endpoint)) {
  state.lock = null; save(); respond({}); process.exit(0);
}
process.stderr.write('unhandled fake GitHub API request: ' + method + ' ' + endpoint + '\\n');
process.exit(2);
`,
    { mode: 0o700 },
  );
  const environment = { ...process.env };
  delete environment.GH_TOKEN;
  delete environment.GITHUB_TOKEN;
  environment.PATH = `${bin}${path.delimiter}${process.env.PATH || ''}`;
  environment.DEOX_FAKE_GH_STATE = statePath;
  environment.DEOX_RELEASE_DRAFT_WAIT_TIMEOUT_MS = '80';
  environment.DEOX_RELEASE_DRAFT_WAIT_POLL_MS = '2';
  environment.DEOX_RELEASE_COMMIT = current;
  environment.GH_REPO_OWNER = 'BurntToasters';
  environment.GH_REPO_NAME = 'deoxidizer';
  return {
    directory,
    environment,
    state() {
      return JSON.parse(fs.readFileSync(statePath, 'utf8'));
    },
    record(event) {
      const state = JSON.parse(fs.readFileSync(statePath, 'utf8'));
      state.events.push(event);
      fs.writeFileSync(statePath, JSON.stringify(state));
    },
    cleanup() {
      fs.rmSync(directory, { recursive: true, force: true });
    },
  };
}

function fakeDraft(commit, extra = {}) {
  return {
    id: 71,
    tag_name: tag,
    draft: true,
    prerelease,
    target_commitish: commit,
    assets: [],
    ...extra,
  };
}

// Failure modes covered before implementation:
// - parallel targets clear or reuse another target's staging/session files;
// - unsigned, ad-hoc, unnotarized, or untimestamped native stages pass upload;
// - proof metadata is stale or its binary/installer digests do not match bytes;
// - Windows npm .cmd assumptions break direct Node release invocation;
// - a missing/false remote digest or missing detached signature bypasses binding;
// - annotated or divergent tag refs differ from the candidate commit;
// - a remote release ID or asset snapshot changes between verification/publish.

test('all six targets receive isolated staging directories', () => {
  const directories = Object.entries(TARGETS).flatMap(([osName, arches]) =>
    Object.values(arches).map((target) => releaseStageDirectory(target, '/release-root')),
  );
  assert.equal(directories.length, 6);
  assert.equal(new Set(directories).size, 6);
  assert.ok(directories.every((directory) => directory.startsWith('/release-root/staging/')));
});

test('same-target builders serialize through an exclusive staging lock', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-release-lock-test-'));
  const stage = path.join(directory, 'staging', TARGETS.linux.x86_64);
  const first = acquireStageLock(stage);
  try {
    assert.throws(() => acquireStageLock(stage), /locked|another build/i);
  } finally {
    first();
  }
  const second = acquireStageLock(stage);
  second();
  fs.rmSync(directory, { recursive: true, force: true });
});

test('an existing remote asset is reusable only when its immutable bytes match', () => {
  const expected = {
    name: 'artifact.tar.gz',
    size: 17,
    digest: `sha256:${'a'.repeat(64)}`,
  };
  assert.equal(validateExistingReleaseAsset(expected, expected), true);
  assert.throws(
    () =>
      validateExistingReleaseAsset(expected, {
        ...expected,
        digest: `sha256:${'b'.repeat(64)}`,
      }),
    /immutable|differs|mismatch/i,
  );
});

test('Windows release npm invocation uses the active Node executable and npm CLI', () => {
  const execPath = 'C:\\Program Files\\nodejs\\node.exe';
  const npmExecPath = 'C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js';
  assert.deepEqual(
    npmReleaseInvocation({
      env: { npm_execpath: npmExecPath },
      platform: 'win32',
      execPath,
    }),
    { command: execPath, args: [npmExecPath] },
  );
});

test('release GitHub token is available only to explicitly authenticated gh calls', () => {
  const environment = { GH_TOKEN: 'secret', GITHUB_TOKEN: 'secret', PATH: '/bin' };
  const scrubbed = githubCliEnvironment(environment);
  assert.equal(scrubbed.GH_TOKEN, undefined);
  assert.equal(scrubbed.GITHUB_TOKEN, undefined);
  const upload = githubCliEnvironment(environment, { allowEnvironmentToken: true });
  assert.equal(upload.GH_TOKEN, 'secret');
  assert.equal(upload.GITHUB_TOKEN, 'secret');
});

test('release mutation lock ref is deterministic and version scoped', () => {
  assert.equal(releaseMutationLockRef('v1.2.3'), 'refs/heads/deoxidizer-release-lock-v1.2.3');
  assert.throws(() => releaseMutationLockRef('../other'), /tag|release/i);
});

test('waiter polls without the shared lock, then rechecks the same draft under lock', () => {
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  const fake = fakeGitHub({ release: fakeDraft(commit) });
  let uploaded = false;
  try {
    withDraftUploadLock(
      tag,
      commit,
      { waitForDraft: true, environment: fake.environment },
      () => {
        assert.equal(fake.state().lock, commit);
        uploaded = true;
        fake.record({ kind: 'upload' });
      },
    );
    assert.equal(uploaded, true);
    const events = fake.state().events;
    const firstList = events.findIndex(
      (event) => event.method === 'GET' && event.endpoint.includes('/releases?'),
    );
    const lock = events.findIndex(
      (event) => event.method === 'POST' && event.endpoint.endsWith('/git/refs'),
    );
    const lockedList = events.findIndex(
      (event, index) =>
        index > lock && event.method === 'GET' && event.endpoint.includes('/releases?'),
    );
    const lockedTag = events.findIndex(
      (event, index) => index > lock && event.method === 'GET' && event.endpoint.includes('/git/ref/tags/'),
    );
    const upload = events.findIndex((event) => event.kind === 'upload');
    assert.ok(firstList >= 0 && firstList < lock, 'wait polling must happen before lock acquisition');
    assert.ok(lock < lockedList && lockedList < lockedTag && lockedTag < upload);
    assert.equal(
      events.some((event) => event.method === 'POST' && event.endpoint.endsWith('/releases')),
      false,
      'waiters must never create a release',
    );
    assert.equal(
      events.some((event) => event.method === 'PATCH' && /\/releases\/\d+$/.test(event.endpoint)),
      false,
      'the locked waiter recheck must not mutate release metadata',
    );
  } finally {
    fake.cleanup();
  }
});

test('a creator creates the draft only after acquiring the shared lock', () => {
  const fake = fakeGitHub();
  const commit = fake.environment.DEOX_RELEASE_COMMIT;
  try {
    withDraftUploadLock(
      tag,
      commit,
      { waitForDraft: false, environment: fake.environment },
      () => {
        assert.equal(fake.state().lock, commit);
        fake.record({ kind: 'upload' });
      },
    );
    const events = fake.state().events;
    const lock = events.findIndex(
      (event) => event.method === 'POST' && event.endpoint.endsWith('/git/refs'),
    );
    const create = events.findIndex(
      (event) => event.method === 'POST' && event.endpoint.endsWith('/releases'),
    );
    const upload = events.findIndex((event) => event.kind === 'upload');
    assert.ok(lock >= 0 && lock < create && create < upload);
    assert.equal(fake.state().release?.id, 73);
  } finally {
    fake.cleanup();
  }
});

test('waiters reject an absent, replaced, published, or rebound draft after locking', () => {
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  for (const mutation of ['absent', 'id', 'published', 'target', 'tag']) {
    const fake = fakeGitHub({ release: fakeDraft(commit), mutateOnLock: mutation });
    let uploaded = false;
    try {
      assert.throws(
        () =>
          withDraftUploadLock(
            tag,
            commit,
            { waitForDraft: true, environment: fake.environment },
            () => {
              uploaded = true;
              fake.record({ kind: 'upload' });
            },
          ),
        /draft|release|commit|tag/i,
        mutation,
      );
      assert.equal(uploaded, false, `${mutation} draft must fail before upload`);
      assert.equal(fake.state().lock, null, `${mutation} lock must be released`);
      assert.equal(
        fake.state().events.some(
          (event) => event.method === 'POST' && event.endpoint.endsWith('/releases'),
        ),
        false,
        `${mutation} waiter must not create a replacement draft`,
      );
    } finally {
      fake.cleanup();
    }
  }
});

test('waiter times out without creating a release when no creator has made one', () => {
  const fake = fakeGitHub();
  const commit = fake.environment.DEOX_RELEASE_COMMIT;
  try {
    assert.throws(
      () =>
        withDraftUploadLock(
          tag,
          commit,
          { waitForDraft: true, environment: fake.environment },
          () => assert.fail('upload must not run'),
        ),
      /timed out/i,
    );
    assert.equal(fake.state().lock, null);
    assert.equal(
      fake.state().events.some(
        (event) => event.method === 'POST' && event.endpoint.endsWith('/releases'),
      ),
      false,
    );
    assert.equal(
      fake.state().events.some(
        (event) => event.method === 'POST' && event.endpoint.endsWith('/git/refs'),
      ),
      false,
    );
  } finally {
    fake.cleanup();
  }
});

test('draft integrity mode verifies remote proof read-only and preserves default draft checks', () => {
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  const assets = [...expectedReleaseAssets()].map((name) => ({ name, size: 1 }));
  const fake = fakeGitHub({ release: fakeDraft(commit), assets });
  const script = path.join(__dirname, '../../scripts/verify-release-draft.cjs');
  try {
    const defaults = spawnSync(process.execPath, [script], {
      encoding: 'utf8',
      env: fake.environment,
    });
    assert.equal(defaults.status, 0, defaults.stderr);
    assert.match(defaults.stdout, /draft .* is complete/i);

    const integrity = spawnSync(process.execPath, [script, '--integrity'], {
      encoding: 'utf8',
      env: fake.environment,
    });
    assert.notEqual(integrity.status, 0);
    assert.match(integrity.stderr, /no valid ID/i);
    const apiEvents = fake.state().events.filter((event) => event.method);
    assert.ok(apiEvents.length > 0);
    assert.equal(
      apiEvents.some((event) => event.method !== 'GET'),
      false,
      'integrity verification must not mutate or publish the release',
    );
  } finally {
    fake.cleanup();
  }
});

test('native proof binds platform evidence to staged bytes and release identity', () => {
  const commit = 'a'.repeat(40);
  const binaries = { deoxidizer: 'b'.repeat(64), deox: 'c'.repeat(64) };
  const mac = {
    schema_version: 1,
    target: 'aarch64-apple-darwin',
    commit,
    platform: 'darwin',
    artifacts: binaries,
    native: {
      mechanism: 'codesign',
      signature_verified: true,
      timestamp_verified: true,
      team_id: 'TEAM123',
      notarization: {
        status: 'Accepted',
        submissions: [
          { asset: 'deoxidizer', submission_id: 'notary-id-one' },
          { asset: 'deox', submission_id: 'notary-id-two' },
        ],
      },
    },
  };
  assert.doesNotThrow(() =>
    validateNativeProof(mac, {
      target: mac.target,
      commit,
      platform: 'darwin',
      binaries,
    }),
  );
  assert.throws(
    () =>
      validateNativeProof(
        { ...mac, native: { ...mac.native, timestamp_verified: false } },
        { target: mac.target, commit, platform: 'darwin', binaries },
      ),
    /timestamp/i,
  );
  assert.throws(
    () =>
      validateNativeProof(
        { ...mac, native: { ...mac.native, notarization: { status: 'skipped', submissions: [] } } },
        { target: mac.target, commit, platform: 'darwin', binaries },
      ),
    /notar/i,
  );
  assert.throws(
    () =>
      validateNativeProof(mac, {
        target: mac.target,
        commit: 'd'.repeat(40),
        platform: 'darwin',
        binaries,
      }),
    /commit/i,
  );
  assert.throws(
    () =>
      validateNativeProof(mac, {
        target: mac.target,
        commit,
        platform: 'darwin',
        binaries: { ...binaries, deoxidizer: '0'.repeat(64) },
      }),
    /digest|hash|artifact/i,
  );
});

test('Windows native proof requires timestamped Authenticode and binds installer bytes', () => {
  const installerName = `deoxidizer-v${packageManifest.version}-windows-x86_64-setup.exe`;
  const installerDigest = 'd'.repeat(64);
  const proof = {
    schema_version: 1,
    target: 'x86_64-pc-windows-msvc',
    commit: 'a'.repeat(40),
    platform: 'windows',
    artifacts: { deoxidizer: 'b'.repeat(64), deox: 'c'.repeat(64) },
    installer: { name: installerName, sha256: installerDigest },
    native: {
      mechanism: 'authenticode',
      signature_verified: true,
      timestamp_verified: true,
      publisher: 'Deoxidizer Publisher',
      subject: 'CN=Deoxidizer Publisher',
    },
  };
  const expected = {
    target: proof.target,
    commit: proof.commit,
    platform: 'windows',
    binaries: proof.artifacts,
    associatedAssets: new Map([[installerName, installerDigest]]),
  };
  assert.doesNotThrow(() => validateNativeProof(proof, expected));
  assert.throws(
    () => validateNativeProof({ ...proof, native: { ...proof.native, timestamp_verified: false } }, expected),
    /timestamp/i,
  );
  assert.throws(
    () =>
      validateNativeProof(proof, {
        ...expected,
        associatedAssets: new Map([[installerName, '0'.repeat(64)]]),
      }),
    /installer|digest|hash/i,
  );
});

test('Linux proof is explicit and still binds both binary digests', () => {
  const proof = {
    schema_version: 1,
    target: 'x86_64-unknown-linux-musl',
    commit: 'a'.repeat(40),
    platform: 'linux',
    artifacts: { deoxidizer: 'b'.repeat(64), deox: 'c'.repeat(64) },
    native: { mechanism: 'none', status: 'not-required' },
  };
  assert.doesNotThrow(() =>
    validateNativeProof(proof, {
      target: proof.target,
      commit: proof.commit,
      platform: 'linux',
      binaries: proof.artifacts,
    }),
  );
});

test('remote verification requires streamed hashes and a signature for every asset', () => {
  const digest = 'a'.repeat(64);
  const assets = [
    { id: 1, name: 'archive.tar.gz', size: 3, digest: `sha256:${digest}` },
    { id: 2, name: 'archive.tar.gz.minisig', size: 2 },
  ];
  const downloaded = new Map([
    ['archive.tar.gz', `sha256:${digest}`],
    ['archive.tar.gz.minisig', `sha256:${'b'.repeat(64)}`],
  ]);
  assert.deepEqual(validateRemoteAssetDigests(assets, downloaded), []);
  assert.match(
    validateRemoteAssetDigests(assets, new Map([['archive.tar.gz', `sha256:${digest}`]])).join('\n'),
    /streamed.*hash|download/i,
  );
  assert.match(
    validateRemoteAssetDigests(
      [{ ...assets[0], digest: 'sha1:bad' }, assets[1]],
      downloaded,
    ).join('\n'),
    /invalid.*digest|sha256/i,
  );
  assert.deepEqual(validateRemoteSignaturePairs(assets), []);
  assert.match(validateRemoteSignaturePairs([assets[0]]).join('\n'), /signature/i);
});

test('release signatures use one centralized filename rule', () => {
  assert.equal(signaturePath('/tmp/artifact'), '/tmp/artifact.minisig');
});

test('Tauri signer wrapper decodes to raw Minisign bytes and secrets stay in environment', () => {
  const raw = Buffer.from(
    'untrusted comment: signature from tauri secret key\n' +
      'RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n' +
      'trusted comment: timestamp:1556193335\tfile:test\n' +
      'y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n',
  );
  assert.deepEqual(decodeTauriSignature(Buffer.from(raw).toString('base64')), raw);
  assert.throws(() => decodeTauriSignature('%%%not base64%%%'), /base64|signature/i);
  assert.throws(() => decodeTauriSignature(Buffer.from('not a Minisign signature').toString('base64')), /signature|format/i);
  const invocation = signerInvocation('/tmp/artifact');
  assert.equal(invocation.command, 'cargo');
  assert.deepEqual(invocation.args, ['tauri', 'signer', 'sign', '/tmp/artifact']);
  assert.ok(!invocation.args.some((argument) => /password|secret|private.key/i.test(argument)));
  const environment = minisignEnvironment({
    TAURI_SIGNING_PRIVATE_KEY_PATH: '/provisioned/key',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'secret',
    PATH: '/bin',
  });
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PATH, '/provisioned/key');
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, 'secret');
  assert.equal(environment.PATH, '/bin');
});

test('Minisign key parser requires a canonical two-line public key with matching key ID', () => {
  const body = Buffer.alloc(42);
  body.write('Ed', 0, 'ascii');
  Buffer.from('0123456789abcdef', 'hex').copy(body, 2);
  Buffer.alloc(32, 7).copy(body, 10);
  const id = Buffer.from(body.subarray(2, 10)).reverse().toString('hex').toUpperCase();
  const publicKey = `untrusted comment: minisign public key: ${id}\n${body.toString('base64')}\n`;
  assert.equal(parsePublicKey(publicKey).keyId, id);
  assert.throws(
    () => parsePublicKey(publicKey.replace(id, 'F'.repeat(16))),
    /key ID|canonical|public key/i,
  );
  assert.throws(() => parsePublicKey(`${publicKey}extra\n`), /two lines|canonical/i);
});

test('release archive checks use the production updater extractor and require proof', () => {
  const source = fs.readFileSync(
    path.join(__dirname, '../../scripts/verify-release.cjs'),
    'utf8',
  );
  assert.match(source, /verify_release_archive/);
  assert.match(source, /--json/);
  assert.match(source, /--require-proof/);
});

test('publication uses a complete remote proof and rechecks its snapshot', () => {
  const source = fs.readFileSync(
    path.join(__dirname, '../../scripts/publish-release.cjs'),
    'utf8',
  );
  assert.match(source, /verifyRemote\(\[\],\s*\{\s*requireComplete:\s*true\s*\}\)/);
  assert.match(source, /requireComplete:\s*true/);
  assert.match(source, /assertRemoteSnapshotUnchanged\(/);
  assert.match(source, /verification\.release\.id/);
  assert.match(source, /withReleaseMutationLock\(/);
});

test('remote release verifier checks actual tag, streamed bytes, every signature, and archive proof', () => {
  const source = fs.readFileSync(
    path.join(__dirname, '../../scripts/verify-release.cjs'),
    'utf8',
  );
  assert.match(source, /resolveTagCommit\(tag\)/);
  assert.match(source, /downloadGitHubAsset\(/);
  assert.match(source, /verifyReleaseSignature/);
  assert.match(source, /validateNativeProof\(/);
  assert.match(source, /assertRemoteSnapshotUnchanged/);
});

test('annotated tag refs peel to and must match the candidate commit', () => {
  const commit = 'a'.repeat(40);
  const tagObject = 'b'.repeat(40);
  const calls = [];
  assert.equal(
    peelTagCommit({ type: 'tag', sha: tagObject }, (sha) => {
      calls.push(sha);
      return { type: 'commit', sha: commit };
    }),
    commit,
  );
  assert.deepEqual(calls, [tagObject]);
  assert.throws(
    () => peelTagCommit({ type: 'commit', sha: 'c'.repeat(40) }, () => null),
    /tag.*commit/i,
  );
  assert.throws(
    () => peelTagCommit({ type: 'tag', sha: tagObject }, () => ({ type: 'tree', sha: commit })),
    /tag.*commit/i,
  );
});

test('draft validation rejects a real tag ref that differs from candidate HEAD', () => {
  const expected = [...expectedReleaseAssets()];
  const assets = expected.map((name) => ({ name, size: 1 }));
  const release = { draft: true, prerelease, tag_name: tag, target_commitish: 'a'.repeat(40) };
  assert.deepEqual(validateDraft(release, assets, 'a'.repeat(40), 'a'.repeat(40)), []);
  assert.match(
    validateDraft(release, assets, 'a'.repeat(40), 'b'.repeat(40)).join('\n'),
    /tag.*commit/i,
  );
  const ensureSource = fs.readFileSync(
    path.join(__dirname, '../../scripts/ensure-draft-release.cjs'),
    'utf8',
  );
  const waitBody = ensureSource.split('async function waitForDraft()')[1]?.split('\nasync function main')[0] || '';
  assert.match(waitBody, /resolveTagCommit\(tag\)/);
});

test('release snapshot detects ID, asset, and tag-commit changes before publication', () => {
  const commit = 'a'.repeat(40);
  const assets = [
    { id: 10, name: 'artifact.tar.gz', size: 4, digest: `sha256:${'b'.repeat(64)}` },
  ];
  const release = { id: 5, tag_name: tag, target_commitish: commit, draft: true, prerelease };
  const snapshot = makeReleaseSnapshot(release, assets, commit);
  assert.doesNotThrow(() => assertSameReleaseSnapshot(snapshot, snapshot));
  assert.throws(
    () => assertSameReleaseSnapshot(snapshot, { ...snapshot, releaseId: 6 }),
    /snapshot|release ID/i,
  );
  assert.throws(
    () =>
      assertSameReleaseSnapshot(snapshot, {
        ...snapshot,
        assets: [{ ...snapshot.assets[0], digest: `sha256:${'c'.repeat(64)}` }],
      }),
    /snapshot|asset/i,
  );
  assert.throws(
    () => assertSameReleaseSnapshot(snapshot, { ...snapshot, tagCommit: 'd'.repeat(40) }),
    /snapshot|tag/i,
  );
});

function storedZip(names) {
  const localParts = [];
  const centralParts = [];
  let localOffset = 0;

  for (const name of names) {
    const filename = Buffer.from(name);
    const local = Buffer.alloc(30 + filename.length);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(filename.length, 26);
    filename.copy(local, 30);
    localParts.push(local);

    const central = Buffer.alloc(46 + filename.length);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(filename.length, 28);
    central.writeUInt32LE(localOffset, 42);
    filename.copy(central, 46);
    centralParts.push(central);
    localOffset += local.length;
  }

  const localData = Buffer.concat(localParts);
  const centralData = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(names.length, 8);
  end.writeUInt16LE(names.length, 10);
  end.writeUInt32LE(centralData.length, 12);
  end.writeUInt32LE(localData.length, 16);
  return Buffer.concat([localData, centralData, end]);
}

test('normalizes release aliases to supported target triples', () => {
  assert.equal(normalizeOs('macos'), 'darwin');
  assert.equal(normalizeOs('win'), 'windows');
  assert.equal(normalizeArch('amd64', 'linux'), 'x86_64');
  assert.equal(normalizeArch('arm64', 'linux'), 'aarch64');
  assert.equal(TARGETS.linux.aarch64, 'aarch64-unknown-linux-musl');
  assert.equal(TARGETS.linux.x86_64, 'x86_64-unknown-linux-musl');
});

test('release scripts match the manual VM draft ownership protocol', () => {
  assert.equal(expectedReleaseAssets().size, 28);
  assert.ok([...expectedReleaseAssets()].some((name) => name.endsWith('.minisig')));
  assert.ok([...expectedReleaseAssets()].every((name) => !name.endsWith('.asc')));
  assert.match(packageManifest.scripts['release:sign'], /scripts\/minisign-sign\.cjs/);
  const releaseSource = fs.readFileSync(
    path.join(__dirname, '../../scripts/release.cjs'),
    'utf8',
  );
  assert.match(releaseSource, /scripts\/minisign-sign\.cjs/);
  assert.doesNotMatch(releaseSource, /gpg-sign|GPG_KEY_ID|GPG_PASSPHRASE/);
  assert.match(packageManifest.scripts['release:windows'], /--upload/);
  assert.doesNotMatch(packageManifest.scripts['release:windows'], /--wait/);
  for (const scriptName of [
    'release:linux',
    'release:linux:x64',
    'release:linux:arm64',
    'release:macos',
    'release:macos:x64',
    'release:macos:arm64',
  ]) {
    assert.match(packageManifest.scripts[scriptName], /--upload/);
    assert.match(packageManifest.scripts[scriptName], /--wait/);
  }
});

test('hosted release workflow requires provisioned self-hosted builders and upload-only token scope', () => {
  const workflow = fs.readFileSync(
    path.join(__dirname, '../../.github/workflows/release.yml'),
    'utf8',
  );
  assert.match(workflow, /runs-on:\s*\[self-hosted,\s*\$\{\{\s*inputs\.runner\s*\}\}\]/);
  assert.doesNotMatch(workflow, /GPG_KEY_ID|GPG_PASSPHRASE/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY_PATH/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY_PASSWORD/);
  assert.match(workflow, /test -f "\$TAURI_SIGNING_PRIVATE_KEY_PATH"/);
  assert.match(workflow, /cargo tauri signer sign --help/);
  assert.doesNotMatch(workflow, /gh auth login/);
  assert.equal((workflow.match(/^\s+GH_TOKEN:/gm) || []).length, 1);
  assert.match(workflow, /Upload staged release[\s\S]*?GH_TOKEN:\s*\$\{\{\s*secrets\.GITHUB_TOKEN\s*\}\}/);
});

test('rejects host architecture for a different operating system', () => {
  const hostOs =
    process.platform === 'darwin'
      ? 'darwin'
      : process.platform === 'win32'
        ? 'windows'
        : 'linux';
  const differentOs = hostOs === 'linux' ? 'darwin' : 'linux';
  assert.throws(() => normalizeArch('host', differentOs), /host architecture/);
});

test('uses a numeric four-part Windows file version for prereleases', () => {
  assert.equal(windowsFileVersion('0.1.0'), '0.1.0.0');
  assert.equal(windowsFileVersion('0.2.0-beta.1'), '0.2.0.0');
  const installer = fs.readFileSync(path.join(__dirname, '../../installer.nsi'), 'utf8');
  assert.match(installer, /VIProductVersion "\$\{FILE_VERSION\}"/);
});

test('parses checksum manifests and rejects duplicates', () => {
  assert.equal(tag, `v${packageManifest.version}`);
  const manifest = parseChecksumManifest(
    `${'a'.repeat(64)}  deoxidizer-v${packageManifest.version}-linux-x86_64.tar.gz\n`,
  );
  assert.equal(manifest.size, 1);
  assert.throws(
    () =>
      parseChecksumManifest(
        `${'a'.repeat(64)}  one\n${'b'.repeat(64)}  one\n`,
      ),
    /Duplicate checksum/,
  );
});

test('verifies local checksum bytes', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'artifact');
  const bytes = Buffer.from('release artifact');
  fs.writeFileSync(filePath, bytes);
  const digest = crypto.createHash('sha256').update(bytes).digest('hex');
  assert.equal(verifyChecksum(filePath, digest), true);
  assert.equal(verifyChecksum(filePath, '0'.repeat(64)), false);
  fs.rmSync(directory, { recursive: true, force: true });
});

test('inspects ZIP archives without an external unzip command', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'release.zip');
  const names = ['deoxidizer.exe', 'deox.exe', 'LICENSE'];
  fs.writeFileSync(filePath, storedZip(names));
  assert.deepEqual(archiveEntries(filePath), new Set(names));
  assert.doesNotThrow(() => verifyZipIntegrity(filePath));
  fs.rmSync(directory, { recursive: true, force: true });
});

test('ZIP integrity checker rejects CRC drift', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-node-test-'));
  const filePath = path.join(directory, 'release.zip');
  const bytes = storedZip(['deoxidizer', 'deox', 'LICENSE']);
  // First central-directory entry starts after three empty local headers.
  const centralOffset = bytes.length - 22 - (46 + 'deoxidizer'.length) - (46 + 'deox'.length) - (46 + 'LICENSE'.length);
  bytes.writeUInt32LE(1, centralOffset + 16);
  fs.writeFileSync(filePath, bytes);
  assert.throws(() => verifyZipIntegrity(filePath), /CRC/);
  fs.rmSync(directory, { recursive: true, force: true });
});

test('scrubs GitHub token environment variables', () => {
  const environment = githubCliEnvironment({
    GH_TOKEN: 'secret',
    GITHUB_TOKEN: 'secret',
    PATH: '/bin',
  });
  assert.equal(environment.GH_TOKEN, undefined);
  assert.equal(environment.GITHUB_TOKEN, undefined);
  assert.equal(environment.PATH, '/bin');
});

test('release build and upload environment excludes signing credentials', () => {
  const environment = buildEnvironment({
    TAURI_SIGNING_PRIVATE_KEY_PATH: '/private/key',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'secret',
    AZURE_CLIENT_SECRET: 'secret',
    APPLE_PASSWORD: 'secret',
    PATH: '/bin',
  });
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PATH, undefined);
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);
  assert.equal(environment.AZURE_CLIENT_SECRET, undefined);
  assert.equal(environment.APPLE_PASSWORD, undefined);
  assert.equal(environment.PATH, '/bin');
});

test('unsigned opt-in reaches only local verification', () => {
  const environment = {
    DEOX_ALLOW_UNSIGNED_RELEASE: '1',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'secret',
    PATH: '/bin',
  };
  const build = buildEnvironment(environment);
  const verification = verificationEnvironment(environment, true);
  assert.equal(build.DEOX_ALLOW_UNSIGNED_RELEASE, undefined);
  assert.equal(verification.DEOX_ALLOW_UNSIGNED_RELEASE, '1');
  assert.equal(verification.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);
  assert.equal(verification.PATH, '/bin');
});

test('release Rust quality commands use the lockfile', () => {
  assert.match(packageManifest.scripts['quality:rust'], /cargo clippy --all-targets --locked/);
  assert.match(packageManifest.scripts['quality:rust'], /cargo test --locked/);
});

test('release identity validation rejects drift and expiry', () => {
  assert.doesNotThrow(() =>
    validateIdentity({ version: '1', commit: 'abc' }, { version: '1', commit: 'abc' }, 'proof'),
  );
  assert.throws(
    () => validateIdentity({ version: '1' }, { version: '2' }, 'proof'),
    /version/,
  );
  assert.throws(
    () => validateFresh({ completedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );
});

test('destructive branch sync requires explicit confirmation', () => {
  const previous = process.env.DEOX_RELEASE_CONFIRM;
  delete process.env.DEOX_RELEASE_CONFIRM;
  assert.throws(() => requireConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
  if (previous === undefined) delete process.env.DEOX_RELEASE_CONFIRM;
  else process.env.DEOX_RELEASE_CONFIRM = previous;
});

test('branch sync --force-always bypasses confirmation gate', () => {
  assert.deepEqual(parseArgs(['main', '--force-always']), {
    branch: 'main',
    forceAlways: true,
  });
  assert.deepEqual(parseArgs(['main']), { branch: 'main', forceAlways: false });
  assert.deepEqual(parseArgs(['--force-always', 'beta']), {
    branch: 'beta',
    forceAlways: true,
  });
  assert.throws(() => parseArgs(['main', '--bogus']), /unknown flag/);
});

test('draft validator requires every supported target manifest and archive', () => {
  const expected = [...expectedReleaseAssets()];
  const assets = expected.map((name) => ({ name, size: 1 }));
  assert.deepEqual(validateDraft({ draft: true, prerelease, tag_name: tag }, assets), []);
  assert.match(
    validateDraft({ draft: true, prerelease, tag_name: tag }, assets.slice(1)).join('\n'),
    /missing asset/,
  );
});

test('draft validator rejects commit drift and unexpected assets', () => {
  const expected = [...expectedReleaseAssets()];
  const assets = expected.map((name) => ({ name, size: 1 }));
  assets.push({ name: 'unexpected.bin', size: 1 });
  const errors = validateDraft(
    { draft: true, prerelease, tag_name: tag, target_commitish: 'b'.repeat(40) },
    assets,
    'a'.repeat(40),
  );
  assert.match(errors.join('\n'), /target commit/);
  assert.match(errors.join('\n'), /unexpected asset/);
});

test('remote validator permits earlier target assets but rejects unknown names', () => {
  const expected = [...expectedReleaseAssets()];
  assert.deepEqual(
    validateRemoteAssetNames(expected.slice(0, 4).map((name) => ({ name }))),
    [],
  );
  assert.match(
    validateRemoteAssetNames([{ name: expected[0] }, { name: 'unexpected.bin' }]).join('\n'),
    /unexpected asset/,
  );
  assert.match(
    validateRemoteAssetNames([{ name: expected[0] }, { name: expected[0] }]).join('\n'),
    /duplicate asset/,
  );
});

test('remote manifest validator binds signed entries to remote asset digests', () => {
  const archive = `deoxidizer-v${tag.slice(1)}-linux-x86_64.tar.gz`;
  const digest = 'a'.repeat(64);
  const assets = new Map([[archive, { name: archive, digest: `sha256:${digest}` }]]);
  assert.deepEqual(
    validateRemoteManifestEntries(
      'SHA256SUMS-linux-x86_64.txt',
      `${digest}  ${archive}\n`,
      assets,
    ),
    [],
  );
  assert.match(
    validateRemoteManifestEntries(
      'SHA256SUMS-linux-x86_64.txt',
      `${'b'.repeat(64)}  ${archive}\n`,
      assets,
    ).join('\n'),
    /digest mismatch/,
  );
});

test('GitHub draft notes come from BCLS changelog', () => {
  assert.equal(tag, `v${packageManifest.version}`);
  const escaped = packageManifest.version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  assert.match(releaseNotes(), new RegExp(`## Changes in \`v${escaped}:\``));
  assert.match(releaseNotes(), /BCLS standard/);
});

// Stage a script under a temp repo root so its __dirname-based `root`
// resolves to a synthetic fixture instead of the real repository. The
// staged copies are never written back to scripts/.
function stageScript(scriptName, files) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-script-test-'));
  const scriptDir = path.join(directory, 'scripts');
  fs.mkdirSync(scriptDir, { recursive: true });
  fs.copyFileSync(
    path.join(__dirname, '../../scripts', scriptName),
    path.join(scriptDir, scriptName),
  );
  for (const [relative, content] of Object.entries(files)) {
    const target = path.join(directory, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
  return directory;
}

function runStagedScript(directory, scriptName, extraEnv) {
  try {
    return spawnSync(process.execPath, [path.join(directory, 'scripts', scriptName)], {
      encoding: 'utf8',
      env: extraEnv === undefined ? process.env : { ...process.env, ...extraEnv },
    });
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test('check-license rejects a fixture with the wrong declared license', () => {
  const licenseHeader = `${' '.repeat(20)}GNU GENERAL PUBLIC LICENSE\n`;
  const good = {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "0.1.0"\nlicense = "GPL-3.0-or-later"\n',
    'package.json': JSON.stringify({ license: 'GPL-3.0-or-later' }),
    'README.md': 'licensed under GPL-3.0-or-later\n',
    'AGENTS.md': 'licensed under GPL-3.0-or-later\n',
    LICENSE: `${licenseHeader}Version 3 text\n`,
  };
  const goodDir = stageScript('check-license.cjs', good);
  assert.equal(runStagedScript(goodDir, 'check-license.cjs').status, 0);

  const badDir = stageScript('check-license.cjs', {
    ...good,
    'Cargo.toml': good['Cargo.toml'].replace('GPL-3.0-or-later', 'MIT'),
  });
  const bad = runStagedScript(badDir, 'check-license.cjs');
  assert.notEqual(bad.status, 0);
  assert.match(bad.stderr, /Cargo\.toml must declare/);
});

test('check-version rejects mismatched fixture versions', () => {
  const directory = stageScript('check-version.cjs', {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "9.9.9"\n',
    'package.json': JSON.stringify({ version: '0.0.0' }),
  });
  const result = runStagedScript(directory, 'check-version.cjs');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not match/);
});

test('check-changelog rejects a fixture missing BCLS markers', () => {
  const directory = stageScript('check-changelog.cjs', {
    'Cargo.toml': '[package]\nname = "deoxidizer"\nversion = "0.1.0"\n',
    'CHANGELOG.md': '# empty changelog\n',
  });
  fs.copyFileSync(
    path.join(__dirname, '../../scripts/sync-version.cjs'),
    path.join(directory, 'scripts/sync-version.cjs'),
  );
  const result = runStagedScript(directory, 'check-changelog.cjs');
  assert.notEqual(result.status, 0);
  assert.match(result.stdout + result.stderr, /missing changelog marker/);
});

// Fixture pins are read from repo files at runtime so toolchain bumps do not
// cause fixture drift. The synthetic scripts/release.cjs snippet mirrors the
// real invocation line asserted below.
function repoToolchainPins() {
  const repoRoot = path.resolve(__dirname, '../..');
  const toolchain = fs
    .readFileSync(path.join(repoRoot, 'rust-toolchain.toml'), 'utf8')
    .match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  const nodePin = fs.readFileSync(path.join(repoRoot, '.node-version'), 'utf8').trim();
  const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, 'package.json'), 'utf8'));
  const npmPin = /npm@([\d.]+)/.exec(packageJson.packageManager ?? '')?.[1];
  const npmEngines = packageJson.engines?.npm;
  const rustVersion = fs
    .readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8')
    .match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];
  assert.ok(toolchain, 'repo rust-toolchain.toml must pin a channel');
  assert.ok(nodePin, 'repo .node-version must pin a version');
  assert.ok(npmPin, 'repo package.json must pin packageManager npm');
  assert.ok(npmEngines, 'repo package.json must declare engines.npm');
  assert.ok(rustVersion, 'repo Cargo.toml must declare rust-version');
  return { toolchain, nodePin, npmPin, npmEngines, rustVersion };
}

function consistentToolchainFixture() {
  const { toolchain, nodePin, npmPin, npmEngines, rustVersion } = repoToolchainPins();
  const sha = 'a'.repeat(40);
  const workflow = (extra = '') =>
    `jobs:\n  build:\n    steps:\n      - uses: dtolnay/rust-toolchain@${sha}\n        with:\n          toolchain: ${toolchain}\n      - uses: actions/setup-node@v4\n        with:\n          node-version: ${nodePin}\n      - run: npm install --global npm@${npmPin}\n${extra}`;
  return {
    'rust-toolchain.toml': `[toolchain]\nchannel = "${toolchain}"\n`,
    'Cargo.toml': `[package]\nname = "deoxidizer"\nversion = "0.1.0"\nrust-version = "${rustVersion}"\n`,
    '.node-version': `${nodePin}\n`,
    'package.json': JSON.stringify({
      packageManager: `npm@${npmPin}`,
      engines: { npm: npmEngines },
    }),
    '.github/workflows/ci.yml': workflow(),
    '.github/workflows/release.yml': workflow(),
    'scripts/release.cjs':
      `run('rustup', ['target', 'add', '--toolchain', '${toolchain}', target], buildEnv);\n`,
  };
}

test('check-toolchain main accepts a consistent fixture, rejects pin drift', () => {
  const { toolchain } = repoToolchainPins();
  // The synthetic snippet must mirror the real scripts/release.cjs invocation line.
  const realRelease = fs.readFileSync(
    path.join(__dirname, '../../scripts/release.cjs'),
    'utf8',
  );
  const escaped = toolchain.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  assert.match(realRelease, new RegExp(`--toolchain',\\s*'${escaped}'`));
  const fixture = consistentToolchainFixture();
  assert.ok(fixture['scripts/release.cjs'].includes(`'${toolchain}'`));
  const goodDir = stageScript('check-toolchain.cjs', fixture);
  const stagedGood = require(path.join(goodDir, 'scripts/check-toolchain.cjs'));
  assert.doesNotThrow(() => stagedGood.main());
  fs.rmSync(goodDir, { recursive: true, force: true });

  const badFiles = consistentToolchainFixture();
  badFiles['.github/workflows/ci.yml'] = badFiles['.github/workflows/ci.yml'].replace(
    `toolchain: ${toolchain}`,
    'toolchain: 0.0.0',
  );
  const badDir = stageScript('check-toolchain.cjs', badFiles);
  try {
    const stagedBad = require(path.join(badDir, 'scripts/check-toolchain.cjs'));
    assert.throws(() => stagedBad.main(), /pinned Rust toolchain/);
  } finally {
    fs.rmSync(badDir, { recursive: true, force: true });
  }
});

test('release key check enforces the canonical public key in runtime and installers', () => {
  const body = Buffer.alloc(42);
  body.write('Ed', 0, 'ascii');
  Buffer.from('0123456789abcdef', 'hex').copy(body, 2);
  Buffer.alloc(32, 7).copy(body, 10);
  const id = Buffer.from(body.subarray(2, 10)).reverse().toString('hex').toUpperCase();
  const publicKey = `untrusted comment: minisign public key: ${id}\n${body.toString('base64')}\n`;
  const encodedKey = JSON.stringify(publicKey);
  const files = {
    'src/minisign.rs': `include_str!("../release-signing-key.pub");\nconst COMMENT = "untrusted comment: minisign public key: ${id}";\nconst BODY = "${body.toString('base64')}";\n`,
    'install.sh': `PINNED_RELEASE_KEY='${encodedKey}'\n`,
    'install.ps1': `$PinnedReleaseKey = '${encodedKey}'\n`,
  };
  assert.doesNotThrow(() => validateKeyPins(publicKey, files));
  const repoRoot = path.resolve(__dirname, '../..');
  const checkedInKey = fs.readFileSync(path.join(repoRoot, 'release-signing-key.pub'), 'utf8');
  assert.equal(parsePublicKey(checkedInKey).keyId, '083B254A48C428E3');
  const checkedInPins = Object.fromEntries(
    ['src/minisign.rs', 'install.sh', 'install.ps1'].map((relative) => [
      relative,
      fs.readFileSync(path.join(repoRoot, relative), 'utf8'),
    ]),
  );
  assert.doesNotThrow(() => validateKeyPins(checkedInKey, checkedInPins));
  assert.throws(
    () => validateKeyPins(publicKey, { ...files, 'install.sh': 'no pinned key\n' }),
    /install\.sh.*pin/i,
  );
  assert.throws(
    () => validateKeyPins(publicKey, { ...files, 'src/minisign.rs': 'no include\n' }),
    /src\/minisign\.rs.*pin/i,
  );
});

test('vi and git-prune refuse without explicit confirmation', () => {
  const previous = process.env.DEOX_RELEASE_CONFIRM;
  delete process.env.DEOX_RELEASE_CONFIRM;
  try {
    assert.throws(() => requireViConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
    assert.throws(() => requirePruneConfirmation(), /DEOX_RELEASE_CONFIRM=YES/);
  } finally {
    if (previous === undefined) delete process.env.DEOX_RELEASE_CONFIRM;
    else process.env.DEOX_RELEASE_CONFIRM = previous;
  }
});

test('publish-release requires --yes before touching the network', () => {
  // No --yes and no confirm env: must fail in the local gate, no gh calls.
  const result = spawnSync(
    process.execPath,
    [path.join(__dirname, '../../scripts/publish-release.cjs')],
    { encoding: 'utf8' },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /--yes/);
});

test('shared prerelease predicate matches only suffixed beta-style versions', () => {
  assert.equal(isPrerelease('0.2.0-beta.1'), true);
  assert.equal(isPrerelease('0.2.0-rc.0'), true);
  assert.equal(isPrerelease('0.2.0-alpha.12'), true);
  assert.equal(isPrerelease('0.1.0'), false);
  assert.equal(isPrerelease('0.2.0-beta'), false);
  assert.equal(isPrerelease('0.2.0-BETA.1'), false);
  const source = fs.readFileSync(
    path.join(__dirname, '../../scripts/publish-release.cjs'),
    'utf8',
  );
  assert.match(source, /isPrerelease\(version\)/);
});

test('release env scoping keeps only OS signing keys', () => {
  // signingEnvironment/minisignEnvironment are intentionally unexported, so
  // load a staged copy with the same appended export instead of editing scripts/.
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-env-test-'));
  const scriptDir = path.join(directory, 'scripts');
  fs.mkdirSync(scriptDir, { recursive: true });
  for (const scriptName of ['release.cjs', 'github-cli.cjs', 'sync-version.cjs']) {
    fs.copyFileSync(
      path.join(__dirname, '../../scripts', scriptName),
      path.join(scriptDir, scriptName),
    );
  }
  const stagedPath = path.join(scriptDir, 'release.cjs');
  fs.appendFileSync(
    stagedPath,
    '\nmodule.exports.signingEnvironment = signingEnvironment;\nmodule.exports.minisignEnvironment = minisignEnvironment;\n',
  );
  let staged;
  try {
    staged = require(stagedPath);
  } catch (error) {
    fs.rmSync(directory, { recursive: true, force: true });
    throw error;
  }
  try {
    const env = {
      PATH: '/bin',
      TAURI_SIGNING_PRIVATE_KEY_PATH: '/key',
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'pass',
      AZURE_CLIENT_SECRET: 'secret',
      APPLE_PASSWORD: 'apple',
      APPLE_ID: 'id',
    };
    const darwin = staged.signingEnvironment(env, 'darwin');
    assert.equal(darwin.APPLE_ID, 'id');
    assert.equal(darwin.AZURE_CLIENT_SECRET, undefined);
    assert.equal(darwin.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);
    assert.equal(darwin.PATH, '/bin');

    const windows = staged.signingEnvironment(env, 'windows');
    assert.equal(windows.AZURE_CLIENT_SECRET, 'secret');
    assert.equal(windows.APPLE_PASSWORD, undefined);
    assert.equal(windows.TAURI_SIGNING_PRIVATE_KEY_PATH, undefined);

    const linux = staged.signingEnvironment(env, 'linux');
    assert.equal(linux.AZURE_CLIENT_SECRET, undefined);
    assert.equal(linux.APPLE_ID, undefined);
    assert.equal(linux.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);

    const minisign = staged.minisignEnvironment(env);
    assert.equal(minisign.TAURI_SIGNING_PRIVATE_KEY_PATH, '/key');
    assert.equal(minisign.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, 'pass');
    assert.equal(minisign.AZURE_CLIENT_SECRET, undefined);
    assert.equal(minisign.APPLE_PASSWORD, undefined);
    assert.equal(minisign.PATH, '/bin');
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('release-session rejects expired proofs and dirty checkouts', () => {
  assert.throws(
    () => validateFresh({ completedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );
  assert.throws(
    () => validateFresh({ startedAt: Date.now() - 2 * 24 * 60 * 60 * 1000 }, 'proof'),
    /expired/,
  );

  // assertCleanCheckout is intentionally unexported; exercise it through a
  // staged copy whose root-relative git/reads point at a temp git repo.
  if (spawnSync('git', ['--version'], { stdio: 'ignore' }).status !== 0) return;
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deoxidizer-git-test-'));
  try {
    const scriptDir = path.join(directory, 'scripts');
    fs.mkdirSync(scriptDir, { recursive: true });
    for (const scriptName of ['release-session.cjs', 'sync-version.cjs']) {
      fs.copyFileSync(
        path.join(__dirname, '../../scripts', scriptName),
        path.join(scriptDir, scriptName),
      );
    }
    const git = (args) =>
      execFileSync('git', ['-c', 'init.defaultBranch=main', ...args], {
        cwd: directory,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    fs.writeFileSync(
      path.join(directory, 'Cargo.toml'),
      '[package]\nname = "deoxidizer"\nversion = "0.1.0"\n',
    );
    fs.writeFileSync(path.join(directory, 'Cargo.lock'), 'fixture\n');
    git(['init']);
    const commitEnv = ['-c', 'user.email=test@example.com', '-c', 'user.name=test'];
    git([...commitEnv, 'add', '-A']);
    git([...commitEnv, '-c', 'commit.gpgsign=false', 'commit', '-m', 'init']);
    assert.equal(
      execFileSync('git', ['status', '--porcelain', '--untracked-files=all'], {
        cwd: directory,
        encoding: 'utf8',
      }).trim(),
      '',
    );

    fs.appendFileSync(path.join(directory, 'Cargo.lock'), 'dirty\n');
    const staged = require(path.join(scriptDir, 'release-session.cjs'));
    assert.throws(() => staged.currentIdentity(), /clean Git checkout/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
