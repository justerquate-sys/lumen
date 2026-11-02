// One careful live smoke test (signed-out): stubbed youtube.com document (no
// HTML page load, which Google rate-limits), but REAL InnerTube API calls, to
// confirm current responses still parse. ~6 API requests. Thumbnails and the
// embed are not loaded. Run: node tests/live-smoke.cjs
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path'), assert = require('node:assert/strict');
const { chromiumOptions, extensionDir } = require('./browser.cjs');
const EXT = extensionDir();

(async () => {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-live-'));
  const ctx = await chromium.launchPersistentContext(profile, {
    headless: true, ...chromiumOptions(), viewport: { width: 1200, height: 800 },
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`],
  });
  const live = [];
  await ctx.route('**/*', route => {
    const req = route.request(), url = new URL(req.url());
    if (url.hostname === 'www.youtube.com' && (url.pathname.startsWith('/youtubei/v1/') || url.pathname === '/sw.js_data')) {
      live.push(url.pathname + (req.postDataJSON()?.continuation ? ' (continuation)' : ''));
      return route.continue();
    }
    if (req.resourceType() === 'document' && url.hostname === 'www.youtube.com' && !url.pathname.startsWith('/embed/')) {
      return route.fulfill({ status: 200, contentType: 'text/html', body: '<!DOCTYPE html><html><head><script>window.__ytAppRan=1</script></head><body></body></html>' });
    }
    return route.abort();
  });
  const page = ctx.pages()[0];
  for (let i = 0; i < 20; i++) {
    await page.goto('https://www.youtube.com/hashtag/warmup', { waitUntil: 'commit' });
    await page.waitForTimeout(300);
    if (await page.locator('#ytc-top').count()) break;
  }
  const t0 = Date.now();
  await page.goto('https://www.youtube.com/results?search_query=roblox+studio+scripting+tutorial', { waitUntil: 'commit' });
  await page.waitForSelector('#ytc-list .c', { timeout: 15000 });
  const searchMs = Date.now() - t0;
  const n = await page.locator('#ytc-list .c').count();
  const titles = await page.locator('#ytc-list .c .t').allTextContents();
  assert.ok(n >= 10, `search cards ${n}`);
  assert.ok(titles.every(t => t.trim().length > 0));
  await page.keyboard.press('Backquote');
  const dbgSearch = await page.locator('#ytc-debug').textContent();
  await page.keyboard.press('Backquote');
  const t1 = Date.now();
  await page.locator('#ytc-list a.t[href^="/watch"]').first().click();
  await page.waitForSelector('#ytc-info h1.wt', { timeout: 15000 });
  await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c:not(.sk)').length > 0, null, { timeout: 15000 });
  const watchMs = Date.now() - t1;
  const watch = await page.evaluate(() => ({
    title: document.querySelector('#ytc-info h1.wt').textContent,
    channel: document.querySelector('#ytc-info .wc')?.textContent,
    stats: document.querySelector('#ytc-info .ws')?.textContent,
    related: document.querySelectorAll('#ytc-list .c:not(.sk)').length,
  }));
  assert.ok(watch.title && watch.channel && watch.related >= 5, JSON.stringify(watch));
  await page.click('#ytc-info a.wc');
  await page.waitForSelector('.ch h1', { timeout: 15000 });
  const channel = { title: await page.locator('.ch h1').textContent(), videos: await page.locator('#ytc-list .c').count() };
  assert.ok(channel.videos >= 10, JSON.stringify(channel));
  await page.keyboard.press('Backquote');
  const dbgChannel = await page.locator('#ytc-debug').textContent();
  console.log(JSON.stringify({ searchCards: n, firstTitles: titles.slice(0, 3), searchMs, watch, watchMs, channel, liveRequests: live }, null, 1));
  console.log('--- search debug\n' + dbgSearch + '\n--- channel debug\n' + dbgChannel);
  assert.equal(await page.evaluate(() => window.__ytAppRan), undefined);
  await ctx.close();
  fs.rmSync(profile, { recursive: true, force: true });
  console.log('live smoke OK');
})().catch(e => { console.error(e); process.exit(1); });
