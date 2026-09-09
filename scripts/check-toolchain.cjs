'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const WORKFLOWS = ['.github/workflows/ci.yml', '.github/workflows/release.yml'];

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), 'utf8');
}

function requiredMatch(text, pattern, label) {
  if (!pattern.test(text)) throw new Error(`${label} does not use the pinned Rust toolchain`);
}

function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] < right[index] ? -1 : 1;
  }
  return 0;
}

function checkRust() {
  const toolchain = read('rust-toolchain.toml').match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  const cargoVersion = read('Cargo.toml').match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1];
  if (!toolchain || !cargoVersion) throw new Error('Rust toolchain declaration is incomplete');

  const numericChannel = toolchain.match(/^\d+\.\d+(?:\.\d+)?$/)?.[0];
  if (numericChannel && !toolchain.startsWith(`${cargoVersion}.`)) {
    throw new Error(`Cargo rust-version ${cargoVersion} does not match channel ${toolchain}`);
  }

  requiredMatch(
    read('.github/workflows/ci.yml'),
    new RegExp(`toolchain:\\s*${toolchain.replaceAll('.', '\\.')}`),
    '.github/workflows/ci.yml',
  );
  requiredMatch(
    read('.github/workflows/release.yml'),
    new RegExp(`toolchain:\\s*${toolchain.replaceAll('.', '\\.')}`),
    '.github/workflows/release.yml',
  );
  requiredMatch(
    read('scripts/release.cjs'),
    new RegExp(`--toolchain',\\s*'${toolchain.replaceAll('.', '\\.')}'`),
    'scripts/release.cjs',
  );
  return `Rust toolchain metadata consistent: ${toolchain}`;
}

function checkNode() {
  const pinned = read('.node-version').trim();
  if (!/^\d+\.\d+\.\d+$/.test(pinned)) {
    throw new Error(`.node-version pin "${pinned}" is not a full version`);
  }
  for (const workflow of WORKFLOWS) {
    const pins = [...read(workflow).matchAll(/node-version:\s*([^\s#'"`]+)/g)].map(
      (match) => match[1],
    );
    if (pins.length === 0) throw new Error(`${workflow} has no node-version pin`);
    for (const pin of pins) {
      if (pin !== pinned) {
        throw new Error(
          `${workflow} pins node-version ${pin} but .node-version is ${pinned} (release agent owns .github/workflows)`,
        );
      }
    }
  }
  return `Node version metadata consistent: ${pinned}`;
}

function checkNpm() {
  const packageJson = JSON.parse(read('package.json'));
  const pinned = /npm@(\d+)\.(\d+)\.(\d+)/
    .exec(packageJson.packageManager ?? '')
    ?.slice(1)
    .map(Number);
  if (!pinned) throw new Error('package.json packageManager does not pin an npm version');
  const engines = packageJson.engines?.npm;
  if (!engines) throw new Error('package.json engines.npm is missing');
  for (const workflow of WORKFLOWS) {
    const installs = [...read(workflow).matchAll(/npm install --global npm@([\d.]+)/g)].map(
      (match) => match[1],
    );
    for (const installed of installs) {
      if (installed !== pinned.join('.')) {
        throw new Error(
          `${workflow} installs npm@${installed} but packageManager pins npm@${pinned.join('.')} (release agent owns .github/workflows)`,
        );
      }
    }
  }
  const lower = />=\s*(\d+)\.(\d+)\.(\d+)/.exec(engines)?.slice(1).map(Number);
  if (!lower) throw new Error(`package.json engines.npm "${engines}" is not a supported >= range`);
  if (compareVersions(pinned, lower) < 0) {
    throw new Error(
      `package.json engines.npm "${engines}" does not admit pinned npm@${pinned.join('.')}`,
    );
  }
  return `npm metadata consistent: packageManager npm@${pinned.join('.')} admitted by engines "${engines}"`;
}

function checkActionShas() {
  const owners = new Map();
  for (const workflow of WORKFLOWS) {
    for (const match of read(workflow).matchAll(/dtolnay\/rust-toolchain@([0-9a-f]{40})/g)) {
      if (!owners.has(match[1])) owners.set(match[1], new Set());
      owners.get(match[1]).add(workflow);
    }
  }
  if (owners.size === 0) {
    throw new Error('No dtolnay/rust-toolchain SHA pins found in workflows');
  }
  if (owners.size > 1) {
    const detail = [...owners.entries()]
      .map(([sha, files]) => `${sha} (${[...files].join(', ')})`)
      .join(' vs ');
    throw new Error(
      `dtolnay/rust-toolchain SHAs drifted between workflows: ${detail} (release agent owns .github/workflows — please sync pins)`,
    );
  }
  return `Action SHA pins consistent: dtolnay/rust-toolchain@${[...owners.keys()][0]}`;
}

function main() {
  const failures = [];
  for (const check of [checkRust, checkNode, checkNpm, checkActionShas]) {
    try {
      console.log(check());
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
  }
  if (failures.length > 0) throw new Error(failures.join('\n'));
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Toolchain check failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

module.exports = { main };
