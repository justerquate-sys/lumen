// A signed-out benchmark profile with a little watch history, so YouTube's Home has a feed (a fresh
// visitor only gets "Try searching to get started"). Seeded once by watching a few public videos on
// YouTube's own page; kept outside the repository (os.tmpdir()/lumen-bench/<name>) and reused.
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { chromiumOptions } = require('../../tests/browser.cjs');

const SEED = ['jNQXAC9IVRw', '9bZkp7q19f0', 'kJQP7kiw5Fk', 'dQw4w9WgXcQ', 'JGwWNGJdvx8'];
// YouTube's cookie consent (accept): without it a signed-out visitor keeps no history at all.
const SOCS = 'CAISNQgDEitib3FfaWRlbnRpdHlmcm9udGVuZHVpc2VydmVyXzIwMjMwODI5LjA3X3AxGgJlbiACGgYIgLC_pwY';

exports.homeIds = j => {
  const out = [];
  const push = id => { if (id && /^[\w-]{11}$/.test(id) && !out.includes(id)) out.push(id); };
  try {
    for (const t of j.contents.twoColumnBrowseResultsRenderer.tabs) {
      const g = t.tabRenderer && t.tabRenderer.content && t.tabRenderer.content.richGridRenderer;
      for (const x of (g && g.contents) || []) {
        const c = x.richItemRenderer && x.richItemRenderer.content;
        if (!c) continue;
        if (c.videoRenderer) push(c.videoRenderer.videoId);
        if (c.lockupViewModel && c.lockupViewModel.contentType === 'LOCKUP_CONTENT_TYPE_VIDEO') push(c.lockupViewModel.contentId);
      }
    }
  } catch {}
  return out;
};

exports.launch = async (dir, ext) => {
  const ctx = await chromium.launchPersistentContext(dir, {
    headless: true, ...chromiumOptions(), viewport: { width: 1400, height: 900 }, locale: 'en-US',
    args: [...(ext ? [`--disable-extensions-except=${ext}`, `--load-extension=${ext}`] : ['--disable-extensions']), '--autoplay-policy=no-user-gesture-required', '--mute-audio'],
  });
  // A persistent consent cookie (a session cookie would be gone in the next run, and with it the history).
  await ctx.addCookies([{ name: 'SOCS', value: SOCS, domain: '.youtube.com', path: '/', secure: true, sameSite: 'Lax', expires: Math.floor(Date.now() / 1000) + 365 * 86400 }]);
  return ctx;
};

// The profile folder for `name`, seeded on first use (a copy of it for `copy` names, so several
// profiles share one history without watching again).
exports.seeded = async (name = 'main', secs = 40) => {
  const root = path.join(os.tmpdir(), 'lumen-bench');
  const dir = path.join(root, name), mark = path.join(dir, '.seeded');
  if (fs.existsSync(mark)) return dir;
  fs.mkdirSync(dir, { recursive: true });
  const ctx = await exports.launch(dir, null);
  const page = ctx.pages()[0] || await ctx.newPage();
  for (const v of SEED) {
    await page.goto(`https://www.youtube.com/watch?v=${v}`, { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(secs * 1000);
    console.error(`seed ${name}: watched ${v} (${await page.evaluate(() => { const m = document.querySelector('video'); return m ? m.currentTime.toFixed(0) + ' s' : 'no video'; })})`);
  }
  await page.goto('https://www.youtube.com/', { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.ytInitialData, null, { timeout: 30000 });
  const n = exports.homeIds(await page.evaluate(() => window.ytInitialData)).length;
  console.error(`seed ${name}: Home has ${n} videos`);
  await ctx.close();
  if (n > 0) fs.writeFileSync(mark, String(Date.now()));
  return dir;
};

if (require.main === module) exports.seeded(process.argv[2] || 'main').then(d => console.log(d));
