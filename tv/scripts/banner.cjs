#!/usr/bin/env node
// Renders the Android TV launcher banner (320×180 dp, written at xhdpi: 640×360 px) from HTML with
// Playwright's Chromium: the mint play glyph of the launcher icon, the app name, "TV" under it.
//   node tv/scripts/banner.cjs [name]     (default "Lumen")
'use strict';
const path = require('node:path');
const { chromium } = require('playwright');
const name = process.argv[2] || 'Lumen';
const out = path.join(__dirname, '..', 'shell', 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'res', 'drawable-xhdpi', 'banner.png');
const esc = s => s.replace(/[&<>"]/g, c => `&#${c.charCodeAt(0)};`);
const html = `<!doctype html><meta charset="utf-8"><style>
html,body{margin:0;width:640px;height:360px;background:#101315}
body{display:flex;align-items:center;justify-content:center;gap:32px;font-family:Inter,"SF Pro Display",system-ui,sans-serif}
.g{width:132px;height:96px;border-radius:26px;background:#b7ebca;display:grid;place-items:center}
.g svg{width:44px;height:44px;margin-left:6px}
.n{display:grid;justify-items:start;margin-top:22px}
.t{color:#f1f3f2;font-size:76px;font-weight:600;letter-spacing:-.02em;line-height:1}
.s{color:#b7ebca;font-size:26px;font-weight:700;letter-spacing:.04em;margin:10px 0 0 4px;line-height:1}
</style><div class="g"><svg viewBox="0 0 44 44"><path d="M4 2 L42 22 L4 42 Z" fill="#101315"/></svg></div><div class="n"><div class="t">${esc(name)}</div><div class="s">TV</div></div>`;
(async () => {
  const browser = await chromium.launch(process.env.LUMEN_EXTENSION_CHROMIUM ? { executablePath: process.env.LUMEN_EXTENSION_CHROMIUM } : {});
  const page = await browser.newPage({ viewport: { width: 640, height: 360 } });
  await page.setContent(html);
  await page.screenshot({ path: out });
  await browser.close();
  console.log(out);
})().catch(e => { console.error(e); process.exit(1); });
