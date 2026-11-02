// One careful LIVE-STREAM run of our player (signed-out): stubbed youtube.com documents but
// REAL /sw.js_data, InnerTube player/next calls, HLS playlists (manifest.googlevideo.com) and
// googlevideo segments. Plays a public live stream ~40 s near the edge, seeks back 20 s (DVR),
// returns with LIVE, and prints latency, playlist refreshes and bytes. Stats pings stay local.
// Run: node tests/live-stream.cjs [liveVideoId]   (find one: a search with the Live filter)
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { chromiumOptions, extensionDir } = require('./browser.cjs');
const EXT = extensionDir();
const VID = process.argv[2] || 'GotlA1KKWoo';

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
      const e = { t: Date.now(), host: url.hostname, path: url.pathname.slice(0, 40), itag: url.searchParams.get('itag'), sq: url.searchParams.get('sq') };
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
    const sample = async label => out.samples.push({ at: Date.now() - t0, label, ...(await st()), cls: await page.getAttribute('#ytc-player', 'class'), time: await page.textContent('#ytc-time') });
    for (let i = 0; i < 6; i++) { await page.waitForTimeout(5000); await sample(); }
    await page.keyboard.press('j'); await page.keyboard.press('j');
    await page.waitForTimeout(4000); await sample('after j j (DVR −20 s)');
    await page.locator('.plive').click();
    await page.waitForTimeout(4000); await sample('after LIVE');
    await page.keyboard.press('Backquote');
    out.debug = (await page.locator('#ytc-debug').textContent()).split('\n').filter(l => /player|live|ms  /.test(l));
  } else {
    await page.goto('https://www.youtube.com/hashtag/after', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-top');
    await page.keyboard.press('Backquote');
    out.debug = (await page.locator('#ytc-debug').textContent()).split('\n').filter(l => /fallback|failed|player|VISIONOS/.test(l));
  }
  const gv = net.filter(e => e.host?.endsWith('googlevideo.com'));
  out.live = { playlists: gv.filter(e => e.host === 'manifest.googlevideo.com').map(e => `${e.path.includes('variant') ? 'master' : 'media'} ${e.status} ${e.bytes} B`),
    segments: gv.filter(e => e.sq).length, segmentBytes: gv.filter(e => e.sq).reduce((a, e) => a + (e.bytes || 0), 0), itags: [...new Set(gv.filter(e => e.sq).map(e => e.itag))] };
  out.network = { total: net.length, players: net.filter(e => e.client).map(e => `${e.client} ${e.status}`), googlevideo: net.filter(e => e.host?.endsWith('googlevideo.com')).length,
    gvStatuses: [...new Set(net.filter(e => e.host?.endsWith('googlevideo.com')).map(e => e.status))], blocked: net.filter(e => e.blocked).map(e => e.host + e.path),
    playerJs: net.filter(e => /\/s\/player\/|base\.js|www-player|widgetapi|iframe_api/.test(e.path || '')), localPings: net.filter(e => e.local).map(e => e.path) };
  console.log(JSON.stringify(out, null, 1));
  await ctx.close();
  fs.rmSync(profile, { recursive: true, force: true });
})().catch(e => { console.error(e); process.exit(1); });
