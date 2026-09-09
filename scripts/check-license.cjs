'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const readme = fs.readFileSync(path.join(root, 'README.md'), 'utf8');
const agents = fs.readFileSync(path.join(root, 'AGENTS.md'), 'utf8');
const license = fs.readFileSync(path.join(root, 'LICENSE'), 'utf8');
const packageJson = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));

const errors = [];
if (!/^license\s*=\s*"GPL-3\.0-or-later"/m.test(cargo)) {
  errors.push('Cargo.toml must declare GPL-3.0-or-later');
}
if (packageJson.license !== 'GPL-3.0-or-later') {
  errors.push('package.json must declare GPL-3.0-or-later');
}
if (!readme.includes('GPL-3.0-or-later')) errors.push('README.md must declare GPL-3.0-or-later');
if (!agents.includes('GPL-3.0-or-later')) errors.push('AGENTS.md must declare GPL-3.0-or-later');
if (!license.startsWith('                    GNU GENERAL PUBLIC LICENSE')) {
  errors.push('LICENSE must contain the GNU GPLv3 text');
}

if (errors.length > 0) {
  console.error(errors.map((error) => `- ${error}`).join('\n'));
  process.exit(1);
}
console.log('License metadata consistent: GPL-3.0-or-later');
