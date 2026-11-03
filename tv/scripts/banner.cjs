#!/usr/bin/env node
// Renders Lumen for TV's launcher art from HTML with Playwright's Chromium.
//
//   node tv/scripts/banner.cjs [name]           banner only (default name "Lumen")
//   node tv/scripts/banner.cjs --icons          launcher icon PNGs only
//   node tv/scripts/banner.cjs --all [name]     both
//
// Banner: 320×180 dp, written at xhdpi (640×360 px) to res/drawable-xhdpi/banner.png: the Lumen mark
// with a faint mint glow behind it and the wordmark beside it, centred as a group on charcoal with
// everything ≥ 10 % inside each edge (launchers crop and round banners; the group sits ~93 px from
// the sides and ~112 px from top and bottom).
//
// Icons (all under tv/shell/src-tauri/): the legacy squircle and round PNGs (mipmap-*/ic_launcher.png,
// ic_launcher_round.png, 48 dp), the adaptive-icon foreground PNGs (mipmap-*/ic_launcher_foreground.png,
// 108 dp, the charcoal glyph alone on transparent; the mint comes from @color/ic_launcher_background)
// and icons/icon.png (512 px). The adaptive icon itself uses the vector drawable-v24/ic_launcher_foreground.xml,
// which holds the same glyph: keep MARK below and that vector in step (glyph scaled 1.5×, offset 18 dp,
// so the 48-unit squircle maps onto the 72 dp visible area and the glyph sits well inside the 66 dp safe zone).
//
// LUMEN_EXTENSION_CHROMIUM, if set, is the Chromium executable to use.
'use strict';
const path = require('node:path');
const { chromium } = require('playwright');

const args = process.argv.slice(2);
const flags = new Set(args.filter(a => a.startsWith('--')));
const name = args.find(a => !a.startsWith('--')) || 'Lumen';
const doBanner = !flags.has('--icons') || flags.has('--all');
const doIcons = flags.has('--icons') || flags.has('--all');

const tauri = path.join(__dirname, '..', 'shell', 'src-tauri');
const res = path.join(tauri, 'gen', 'android', 'app', 'src', 'main', 'res');

const MINT = '#b7ebca', INK = '#101315', TEXT = '#edf3f3';
// The Lumen mark, viewBox 0 0 48 48 (the in-app rail logo uses exactly this).
const GLYPH = `<path d="M17.5 12.5v21h14" fill="none" stroke="${INK}" stroke-width="6.5" stroke-linecap="round" stroke-linejoin="round"/><circle cx="32" cy="15.5" r="4" fill="${INK}"/>`;
const MARK = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect width="48" height="48" rx="14" fill="${MINT}"/>${GLYPH}</svg>`;

const esc = s => s.replace(/[&<>"]/g, c => `&#${c.charCodeAt(0)};`);
const page0 = (w, h, body, css = '') => `<!doctype html><meta charset="utf-8"><style>
html,body{margin:0;width:${w}px;height:${h}px;overflow:hidden;background:transparent}
svg{display:block}${css}</style>${body}`;

// The wordmark is system-ui (SF on macOS; Inter and "SF Pro Display" are not installed and would fall
// back to Times in Playwright's Chromium). No "TV" label: it vanished at launcher-row size.
const banner = page0(640, 360, `<div class="b"><div class="m">${MARK}</div><div class="w">${esc(name)}</div></div>`, `
html,body{background:${INK}}
body{display:flex;align-items:center;justify-content:center}
.b{position:relative;z-index:0;display:flex;align-items:center;gap:32px}
.m{position:relative}
.m::before{content:"";position:absolute;inset:-150px;z-index:-1;
  background:radial-gradient(closest-side,rgba(183,235,202,.14),rgba(183,235,202,0))}
.m svg{width:136px;height:136px}
.w{color:${TEXT};font:600 100px/1 system-ui,"Helvetica Neue",Arial,sans-serif;letter-spacing:-.035em}`);

// Icon pages, each `size` px square.
const icon = (size, kind) => {
  if (kind === 'squircle') return page0(size, size, MARK.replace('<svg ', `<svg width="${size}" height="${size}" `));
  if (kind === 'round') return page0(size, size, `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 48 48"><circle cx="24" cy="24" r="24" fill="${MINT}"/>${GLYPH}</svg>`);
  // adaptive foreground: 108 dp canvas, 48 units → 72 dp
  return page0(size, size, `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="-18 -18 108 108"><g transform="scale(1.5)">${GLYPH}</g></svg>`);
};

(async () => {
  const browser = await chromium.launch(process.env.LUMEN_EXTENSION_CHROMIUM ? { executablePath: process.env.LUMEN_EXTENSION_CHROMIUM } : {});
  const shot = async (w, h, html, out, transparent) => {
    const page = await browser.newPage({ viewport: { width: w, height: h } });
    await page.setContent(html);
    await page.screenshot({ path: out, omitBackground: !!transparent });
    await page.close();
    console.log(out);
  };
  if (doBanner) await shot(640, 360, banner, path.join(res, 'drawable-xhdpi', 'banner.png'));
  if (doIcons) {
    for (const [d, s] of [['mdpi', 48], ['hdpi', 72], ['xhdpi', 96], ['xxhdpi', 144], ['xxxhdpi', 192]]) {
      const dir = path.join(res, `mipmap-${d}`);
      await shot(s, s, icon(s, 'squircle'), path.join(dir, 'ic_launcher.png'), true);
      await shot(s, s, icon(s, 'round'), path.join(dir, 'ic_launcher_round.png'), true);
      const f = s * 108 / 48;
      await shot(f, f, icon(f, 'foreground'), path.join(dir, 'ic_launcher_foreground.png'), true);
    }
    await shot(512, 512, icon(512, 'squircle'), path.join(tauri, 'icons', 'icon.png'), true);
  }
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
