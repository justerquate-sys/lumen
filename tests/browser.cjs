// Shared test helpers: which Chromium to drive and which extension folder to load.
'use strict';
const fs = require('node:fs'), path = require('node:path');

// LUMEN_EXTENSION_CHROMIUM: any full Chrome/Chromium that loads unpacked extensions. Unset:
// Playwright's own Chromium (`npx playwright install chromium`), in its extension-capable headless mode.
exports.chromiumOptions = () => process.env.LUMEN_EXTENSION_CHROMIUM
  ? { executablePath: process.env.LUMEN_EXTENSION_CHROMIUM }
  : { channel: 'chromium' };

// The extension the tests load: extension/ (or YTC_EXT, e.g. another build for timing comparisons).
exports.extensionDir = () => path.resolve(process.env.YTC_EXT || path.join(__dirname, '..', 'extension'));

// Development build (./build.sh --dev): its manifest grants the local debug-center collector.
exports.isDevBuild = dir => (JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8')).host_permissions || [])
  .some(p => p.startsWith('http://127.0.0.1:47321/'));
