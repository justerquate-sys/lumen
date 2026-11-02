// One careful live own-player run (signed-out): stubbed youtube.com documents (no HTML page
// loads, which Google rate-limits) but REAL /sw.js_data, InnerTube player/next calls and
// googlevideo media. Plays a public video ≥ 30 s with a seek past 60 s of media (where
// PO-token-less clients get cut off), then stops. Stats pings are answered locally (not sent).
// Run: node tests/live-player.cjs [videoId]
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { chromiumOptions, extensionDir } = require('./browser.cjs');
const EXT = extensionDir();
const VID = process.argv[2] || 'aqz-KE-bpKQ';

(async () => {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-live-player-'));
  const ctx = await chromium.launchPersistentContext(profile, {
    headless: true, ...chromiumOptions(), viewport: { width: 1400, height: 900 },
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, '--autoplay-policy=no-user-gesture-required'],
  });
  const net = [];
  await ctx.route('**/*', async route => {
    const req = route.request(), url = new URL(req.url());
    const yt = url.hostname === 'www.youtube.com';
    if ((yt && (url.pathname.startsWith('/youtubei/v1/') || url.pathname === '/sw.js_data' || url.pathname === '/api/timedtext')) || url.hostname.endsWith('.googlevideo.com')) {
      const e = { t: Date.now(), host: url.hostname, path: url.pathname, itag: url.searchParams.get('itag'), range: url.searchParams.get('range') };
      if (url.pathname === '/youtubei/v1/player') e.client = req.postDataJSON()?.context?.client?.clientName;
      net.push(e);
      try { const res = await route.fetch(); e.status = res.status(); e.bytes = (await res.body()).length; return route.fulfill({ response: res }); }
      catch (err) { e.status = 'error ' + err.message; return route.abort(); }
    }
    if (yt && url.pathname.startsWith('/api/stats/')) { net.push({ t: Date.now(), path: url.pathname, local: true }); return route.fulfill({ status: 204, body: '' }); }
    if (req.resourceType() === 'document' && yt) {
      return route.fulfill({ status: 200, contentType: 'text/html', body: '<!DOCTYPE html><html><head><script>window.__ytAppRan=1</script></head><body>native stub</body></html>' });
    }
    net.push({ t: Date.now(), host: url.hostname, path: url.pathname, blocked: true });
    return route.abort();
  });
  const page = ctx.pages()[0];
  for (let i = 0; i < 20; i++) {
    await page.goto('https://www.youtube.com/hashtag/warmup', { waitUntil: 'commit' });
    await page.waitForTimeout(300);
    if (await page.locator('#ytc-top').count()) break;
  }
  await page.waitForTimeout(1500); // let the sw.js_data config refresh land (visitor data)
  net.length = 0;
  const st = () => page.evaluate(() => { const v = document.getElementById('ytc-video'); if (!v) return { url: location.href };
    const q = v.getVideoPlaybackQuality(); return { t: +v.currentTime.toFixed(2), paused: v.paused, rs: v.readyState, w: v.videoWidth, h: v.videoHeight, dropped: q.droppedVideoFrames, frames: q.totalVideoFrames }; });
  const out = { videoId: VID, samples: [] };
  await page.goto(`https://www.youtube.com/watch?v=${VID}`, { waitUntil: 'commit' });
  try {
    await page.waitForFunction(() => document.documentElement.dataset.ytcFirstFrame || document.querySelector('#ytc-player.failed') || location.search.includes('yt-native'), null, { timeout: 15000 });
  } catch { out.timeout = true; }
  out.url = page.url();
  out.firstFrameMs = await page.evaluate(() => +document.documentElement.dataset.ytcFirstFrame || null).catch(() => null);
  if (!out.url.includes('yt-native') && out.firstFrameMs) {
    const t0 = Date.now();
    for (let i = 0; i < 4; i++) { await page.waitForTimeout(4000); out.samples.push({ at: Date.now() - t0, ...(await st()) }); }
    await page.evaluate(() => { document.getElementById('ytc-video').currentTime = 300; });
    await page.waitForTimeout(2000); out.samples.push({ at: Date.now() - t0, label: 'after seek to 300', ...(await st()) });
    for (let i = 0; i < 4; i++) { await page.waitForTimeout(4000); out.samples.push({ at: Date.now() - t0, ...(await st()) }); }
    await page.keyboard.press('Backquote');
    out.debug = (await page.locator('#ytc-debug').textContent()).split('\n').filter(l => /player|ms  /.test(l));
  } else {
    await page.goto('https://www.youtube.com/hashtag/after', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-top');
    await page.keyboard.press('Backquote');
    out.debug = (await page.locator('#ytc-debug').textContent()).split('\n').filter(l => /fallback|failed|player|VISIONOS/.test(l));
  }
  out.network = { total: net.length, players: net.filter(e => e.client).map(e => `${e.client} ${e.status}`), googlevideo: net.filter(e => e.host?.endsWith('googlevideo.com')).length,
    gvStatuses: [...new Set(net.filter(e => e.host?.endsWith('googlevideo.com')).map(e => e.status))], blocked: net.filter(e => e.blocked).map(e => e.host + e.path),
    playerJs: net.filter(e => /\/s\/player\/|base\.js|www-player|widgetapi|iframe_api/.test(e.path || '')), localPings: net.filter(e => e.local).map(e => e.path) };
  console.log(JSON.stringify(out, null, 1));
  await ctx.close();
  fs.rmSync(profile, { recursive: true, force: true });
})().catch(e => { console.error(e); process.exit(1); });
