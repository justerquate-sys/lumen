// Fails unless extension/ holds the public build: run before committing (the development build
// from ./build.sh --dev writes the same files). Usage: npm run check:public
'use strict';
const fs = require('node:fs'), path = require('node:path');
const { isDevBuild } = require('./browser.cjs');
const dir = path.join(__dirname, '..', 'extension');
const problems = [];
if (isDevBuild(dir)) problems.push('manifest.json grants the local debug-center collector (development build)');
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
if (manifest.version_name || / \(dev\)$/.test(manifest.name)) problems.push('manifest.json carries a development name or build id');
if (fs.existsSync(path.join(dir, 'build-id.txt'))) problems.push('build-id.txt exists (development build)');
for (const f of ['content.js', 'background.js']) {
  const text = fs.readFileSync(path.join(dir, f), 'utf8');
  for (const s of ['127.0.0.1:47321', 'ytc-dev-check', 'build-id.txt', 'ytcIncidentsV1', 'ytcPerfQueueV1', '@dev-start'])
    if (text.includes(s)) problems.push(`${f} contains "${s}"`);
}
const pkg = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'package.json'), 'utf8'));
const cargo = /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(__dirname, '..', 'Cargo.toml'), 'utf8'))[1];
if (manifest.version !== pkg.version || manifest.version !== cargo) problems.push(`versions differ: manifest ${manifest.version}, package.json ${pkg.version}, Cargo.toml ${cargo}`);
// Nothing secret or machine-specific is tracked: signing keys and keystores (the TV app's live in
// ~/.config/lumen-tv/), or node_modules (once committed as a symlink to a personal path).
try {
  const tracked = require('node:child_process').execFileSync('git', ['ls-files', '-z'], { cwd: path.join(__dirname, '..'), encoding: 'utf8' }).split('\0').filter(Boolean);
  for (const f of tracked) if (/(^|\/)(node_modules|keystore\.properties)$|\.(jks|keystore|pem|p12)$/.test(f)) problems.push(`${f} is tracked (keys, keystores and node_modules never are)`);
} catch {} // (not a git checkout: nothing to check)
if (problems.length) { console.error('extension/ is not a clean public build:\n- ' + problems.join('\n- ') + '\nRun ./build.sh (without --dev).'); process.exit(1); }
console.log('extension/ is the public build (' + manifest.version + ').');
