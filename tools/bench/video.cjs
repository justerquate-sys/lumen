// Live video-path benchmark (development builds; real youtube.com, signed out). It measures, from the
// client's own speed telemetry (docs/debug-center.md), how fast our player gets from the user's choice
// to the first frame and to HD, how fast a seek and a quality change show, and prints p50/p90 per
// scenario. Live network only here: the e2e suite stays offline.
//
//   ./build.sh --dev && node tools/bench/video.cjs [--ext DIR] [--runs N] [--videos id,id,…]
//       [--scenarios cold,spa,hover,quick] [--label NAME] [--out FILE.json] [--brave] [--starts]
// (--starts: starts only, no seek or quality change; much quicker)
//
// Slow YouTube (2 Nov 2026: "if requests to YouTube would slow down like 100 times, we should have
// a workaround, but still connecting with YouTube"):
//   --throttle F --throttle-mode node|urls|all [--play S]
//       googlevideo goes through a local HTTPS proxy (the browser maps *.googlevideo.com to it, QUIC
//       off) that streams the real responses F times slower: headers and every chunk arrive F times
//       later than they came from YouTube (a real drip, not one late delivery). `node`: only the URL's
//       own host is slow (its mirrors are not: a slow node); `urls`: every request with the first
//       player response's URLs (any host) is slow (throttled URLs; fresh ones are not); `all`:
//       everything (a slow connection). `--throttle 1` is the proxy alone (the control). The video
//       then plays S seconds (default 25) with no interaction; stall time and quality are reported.
//   --hang-player   the first VISIONOS request of every video never answers (the hedge must win).
//
// Each (run, i) gets a fresh browser profile (cold HTTP cache, DNS and connections) and plays three
// different videos of the fixed list, so no video is ever served from the cache:
//   cold   videos[i]   a watch URL opened as a new document (the client boots on it)
//   spa    videos[i+1] a click from Home on a link that was never hovered (no prefetch)
//   hover  videos[i+2] a click from Home after the pointer rested on the link for 1 s (prefetched)
//   quick  videos[i+3] a click from Home 120 ms after the pointer came onto the link (prefetch in flight)
// In each: first frame, ≥ 1080p on screen, then three seeks (`video.seek`, input → playing): to the
// middle (a click on the seek bar: outside the buffer), 6 s ahead (inside it), and to a quarter after
// the pointer rested there for 0.6 s (outside the buffer, the seek bar's hover known in advance),
// then Quality → 720 and back to Auto (each `video.switch`: decision → the new resolution on screen).
// `play` = first frame (`loadeddata`) → the picture moving (a presented frame 50 ms of media later); `stall_ms` = the session's time waiting for data.
// YouTube's HTML documents are stubbed (Google rate-limits page loads; the client never needs them);
// InnerTube, googlevideo and i.ytimg go to the network untouched; stats pings and SponsorBlock are
// answered locally. Telemetry goes to a collector inside this script, never to the debug center.
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path'), http = require('node:http'), https = require('node:https');
const { execFileSync } = require('node:child_process');

const arg = (name, def) => { const i = process.argv.indexOf('--' + name); return i < 0 ? def : process.argv[i + 1]; };
const flag = name => process.argv.includes('--' + name);
// Several builds (--ext a,b): interleaved per round (alternating order), so they meet the same
// network conditions (live timings drift by ±30 % over a quarter of an hour).
const EXTS = arg('ext', path.join(__dirname, '..', '..', 'extension')).split(',').map(e => path.resolve(e));
let EXT = EXTS[0];
const RUNS = +arg('runs', 2);
const LABELS = (arg('label', '') || EXTS.map(e => path.basename(e)).join(',')).split(',');
const OUT = arg('out', '');
const THROTTLE = +arg('throttle', 0);
const TMODE = arg('throttle-mode', 'node');
const PLAY = +arg('play', 25);
const HANG = flag('hang-player');
const SCEN = arg('scenarios', THROTTLE || HANG ? 'cold,hover' : 'cold,spa,hover').split(',');
// Public videos of different lengths, ladders and codecs (all reach ≥ 1080p).
const VIDEOS = arg('videos', [
  'aqz-KE-bpKQ', // Big Buck Bunny 60 fps, 10:34, up to 2160p60
  'LXb3EKWsInQ', // Costa Rica 4K 60 fps HDR, 5:13
  'eRsGyueVLvQ', // Sintel, 14:48
  'dQw4w9WgXcQ', // music video, 3:33, 1080p
  'fJ9rUzIMcZQ', // music video, 5:59
  'iG9CE55wbtY', // TED talk, 20 min
  'OPf0YbXqDm0', // music video, 4:30
  'hT_nvWreIhg', // music video, 4:43
  'RgKAFK5djSk', // music video, 3:57
  'kJQP7kiw5Fk', // music video, 4:42
].join(',')).split(',');

for (const e of EXTS) if (!JSON.parse(fs.readFileSync(path.join(e, 'manifest.json'), 'utf8')).host_permissions.some(p => p.includes('127.0.0.1:47321'))) {
  console.error(`${e} is not a development build (./build.sh --dev): it has no speed telemetry.`);
  process.exit(2);
}

const pct = (a, q) => { const s = a.filter(Number.isFinite).sort((x, y) => x - y); if (!s.length) return NaN; return s[Math.min(s.length - 1, Math.floor(q * (s.length - 1) + 0.5))]; };
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function collector() {
  const batches = [];
  const srv = http.createServer((req, res) => {
    let body = ''; req.on('data', c => body += c); req.on('end', () => {
      res.writeHead(req.method === 'OPTIONS' ? 204 : 200, { 'access-control-allow-origin': '*', 'access-control-allow-headers': 'content-type', 'access-control-allow-private-network': 'true' });
      if (req.method === 'POST') { try { const j = JSON.parse(body); if (j.kind === 'perf') batches.push(j); else batches.push({ incident: j }); } catch {} }
      res.end('ok');
    });
  }).listen(0, '127.0.0.1');
  await new Promise(r => srv.once('listening', r));
  return { srv, batches, url: `http://127.0.0.1:${srv.address().port}/incident` };
}

// googlevideo, F times slower (see the header). Returns the port and counters.
async function gvProxy(factor, mode) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lumen-bench-tls-'));
  const key = path.join(dir, 'k.pem'), cert = path.join(dir, 'c.pem');
  execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', cert, '-days', '1', '-subj', '/CN=googlevideo.com', '-addext', 'subjectAltName=DNS:*.googlevideo.com'], { stdio: 'ignore' });
  const firstEi = new Map(), stats = { reqs: 0, slow: 0 };
  const agent = new https.Agent({ keepAlive: true, maxSockets: 64 });
  const srv = https.createServer({ key: fs.readFileSync(key), cert: fs.readFileSync(cert) }, (req, res) => {
    const host = (req.headers.host || '').split(':')[0];
    const u = new URL(req.url, `https://${host}`);
    const id = u.searchParams.get('id'), ei = u.searchParams.get('ei');
    if (id && ei && !firstEi.has(id)) firstEi.set(id, ei);
    const slow = factor > 1 && (mode === 'all' || (mode === 'node' && !u.searchParams.has('fallback_count')) || (mode === 'urls' && id && firstEi.get(id) === ei));
    stats.reqs++; if (slow) stats.slow++;
    const t0 = Date.now(), due = t => slow ? t0 + factor * (t - t0) : t;
    const headers = { ...req.headers, host };
    delete headers.connection;
    let chain = Promise.resolve(), gone = false;
    const later = (t, f) => { chain = chain.then(() => sleep(Math.max(0, due(t) - Date.now()))).then(() => { if (!gone) f(); }); };
    const up = https.request({ hostname: host, servername: host, path: req.url, method: req.method, headers, agent }, ur => {
      const h = { ...ur.headers }; delete h.connection; delete h['keep-alive'];
      later(Date.now(), () => res.writeHead(ur.statusCode, h));
      ur.on('data', c => later(Date.now(), () => res.write(c)));
      ur.on('end', () => later(Date.now(), () => res.end()));
    });
    up.on('error', () => { if (!gone) res.destroy(); });
    res.on('close', () => { gone = true; up.destroy(); });
    req.pipe(up);
  }).listen(0, '127.0.0.1');
  await new Promise(r => srv.once('listening', r));
  return { srv, stats, port: srv.address().port, dir };
}
let PROXY = null;

async function browser(col) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'lumen-bench-video-'));
  const opts = flag('brave') ? { executablePath: '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser' } : { channel: 'chromium' };
  const ctx = await chromium.launchPersistentContext(profile, {
    headless: true, ...opts, viewport: { width: 1400, height: 900 },
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, '--autoplay-policy=no-user-gesture-required',
      ...(PROXY ? [`--host-resolver-rules=MAP *.googlevideo.com 127.0.0.1:${PROXY.port}`, '--disable-quic', '--ignore-certificate-errors',
        // The mapped host resolves to 127.0.0.1: without this, Local Network Access blocks it.
        '--disable-features=LocalNetworkAccessChecks,BlockInsecurePrivateNetworkRequests,PrivateNetworkAccessSendPreflights,PrivateNetworkAccessRespectPreflightResults'] : [])],
    ...(PROXY ? { ignoreHTTPSErrors: true } : {}),
  });
  const visSeen = new Set();
  await ctx.route('https://www.youtube.com/**', route => {
    const req = route.request(), u = new URL(req.url());
    if (req.resourceType() === 'document') return route.fulfill({ status: 200, contentType: 'text/html', body: '<!DOCTYPE html><html><head></head><body></body></html>' });
    if (u.pathname.startsWith('/api/stats/') || u.pathname.startsWith('/ptracking')) return route.fulfill({ status: 204, body: '' });
    if (HANG && u.pathname === '/youtubei/v1/player') {
      let b = null; try { b = req.postDataJSON(); } catch {}
      if (b?.context?.client?.clientName === 'VISIONOS' && !visSeen.has(b.videoId)) {
        visSeen.add(b.videoId);
        return sleep(20000).then(() => route.abort().catch(() => {}));
      }
    }
    return route.continue();
  });
  await ctx.route('https://sponsor.ajay.app/**', route => route.fulfill({ status: 404, body: '' }));
  // The page's own view of the picture: when a frame first showed and at which height, every resize.
  await ctx.addInitScript(() => {
    const pics = window.__benchPics = [];
    for (const ev of ['loadeddata', 'resize']) document.addEventListener(ev, e => { const v = e.target; if (v && v.id === 'ytc-video') pics.push({ ev, t: performance.now(), h: v.videoHeight }); }, true);
    // The picture first moving after the first frame: a presented frame 50 ms of media past the first one.
    document.addEventListener('loadstart', e => {
      const v = e.target; if (!v || v.id !== 'ytc-video' || v.__benchRvfc || !v.requestVideoFrameCallback) return;
      v.__benchRvfc = true; let m0 = null;
      const cb = (now, md) => { if (m0 == null) m0 = md.mediaTime; if (md.mediaTime > m0 + 0.05) { window.__benchMove = window.__benchMove || now; return; } v.requestVideoFrameCallback(cb); };
      v.requestVideoFrameCallback(cb);
    }, true);
  });
  const sw = ctx.serviceWorkers()[0] || await ctx.waitForEvent('serviceworker');
  await sw.evaluate(u => chrome.storage.local.set({ ytcCollectorV1: u }), col.url);
  const page = ctx.pages()[0] || await ctx.newPage();
  for (let i = 0; i < 20; i++) {
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForTimeout(300);
    if (await page.locator('#ytc-top').count()) break;
  }
  await page.waitForTimeout(1500); // the config refresh (visitor data) lands
  return { ctx, page, profile };
}

// Waits for this video's `video.session` (sent when the page is left) and returns its events.
async function eventsFor(col, from, vid) {
  for (let i = 0; i < 80; i++) {
    const ev = col.batches.slice(from).flatMap(b => b.events || []);
    if (ev.some(e => e.m === 'video.session' && e.vid === vid)) return ev.filter(e => e.vid === vid);
    await sleep(100);
  }
  return col.batches.slice(from).flatMap(b => b.events || []).filter(e => e.vid === vid);
}

async function played(page, timeout) {
  return page.waitForFunction(() => document.documentElement.dataset.ytcFirstFrame || document.querySelector('#ytc-player.failed, .pfail'), null, { timeout }).then(() => true, () => false);
}

// After the first frame: wait for ≥ 1080p (or 8 s), seek to the middle, Quality 720 → Auto.
async function interact(page, ev) {
  await page.waitForFunction(() => (window.__benchPics || []).some(p => p.h >= 1080), null, { timeout: 8000 }).catch(() => {});
  await page.waitForTimeout(1500);
  const box = await page.locator('#ytc-seek').boundingBox().catch(() => null);
  const settle = async () => {
    await page.waitForFunction(() => { const v = document.getElementById('ytc-video'); return v && !v.seeking && v.readyState >= 3 && !v.paused; }, null, { timeout: 8000 }).catch(() => {});
    await page.waitForTimeout(1500);
  };
  if (box) {
    const y = box.y + box.height / 2;
    // Outside the buffer: the middle.
    await page.mouse.move(box.x + box.width / 2, y);
    await page.mouse.down(); await page.mouse.up();
    await settle();
    // Inside the buffer: 6 s ahead.
    const f = await page.evaluate(() => { const v = document.getElementById('ytc-video'); return v && v.duration > 0 ? (v.currentTime + 6) / v.duration : 0.55; });
    await page.mouse.move(box.x + box.width * Math.min(0.98, f), y);
    await page.mouse.down(); await page.mouse.up();
    await settle();
    // Outside the buffer after the pointer rested on the spot.
    await page.mouse.move(box.x + box.width * 0.25, y, { steps: 4 });
    await page.waitForTimeout(600);
    await page.mouse.down(); await page.mouse.up();
    await settle();
  }
  for (const q of ['720', '0']) {
    await page.mouse.move(5, 5);
    await page.waitForTimeout(500);
    await page.click('#ytc-qual').catch(() => {});
    const row = page.locator(`#ytc-menu [data-act="p-set"][data-arg="q:${q}"]`);
    await row.click({ timeout: 2000 }).catch(e => ev.notes.push('quality ' + q + ': ' + e.message.split('\n')[0]));
    // A switch shows at the next segment boundary at least 1 s ahead (segments are ~5 s).
    await page.waitForTimeout(8000);
  }
}

async function scenario(b, col, kind, vid) {
  const { page } = b;
  const out = { kind, vid, notes: [] };
  const from = col.batches.length;
  if (kind === 'cold') {
    await page.goto(`https://www.youtube.com/watch?v=${vid}`, { waitUntil: 'commit' });
  } else {
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-top', { timeout: 15000 });
    await page.waitForTimeout(800);
    await page.evaluate(v => {
      const a = document.createElement('a'); a.href = `/watch?v=${v}`; a.id = 'bench-link'; a.textContent = 'bench video';
      a.style.cssText = 'position:fixed;left:300px;top:300px;width:300px;height:170px;z-index:2147483647;background:#333;display:block';
      document.body.appendChild(a);
    }, vid);
    if (kind === 'spa') {
      await page.evaluate(() => document.getElementById('bench-link').dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, detail: 1, clientX: 400, clientY: 400 })));
    } else {
      await page.mouse.move(400, 380);
      await page.waitForTimeout(kind === 'hover' ? 1000 : 120);
      await page.mouse.down(); await page.mouse.up();
    }
  }
  const ok = await played(page, THROTTLE ? 90000 : 20000);
  if (!ok) out.notes.push("no first frame");
  if (ok && (THROTTLE || HANG)) await page.waitForTimeout(PLAY * 1000);
  else if (ok && !flag('starts')) await interact(page, out);
  else if (ok) await page.waitForTimeout(2500);
  out.pics = await page.evaluate(() => window.__benchPics || []).catch(() => []);
  out.move_at = await page.evaluate(() => window.__benchMove || null).catch(() => null);
  out.failed = await page.locator('#ytc-player.failed, .pfail').count().catch(() => 0) > 0;
  // Leaving sends the batch.
  await page.goto('https://www.youtube.com/hashtag/bench', { waitUntil: 'commit' }).catch(() => {});
  const ev = await eventsFor(col, from, vid);
  for (const b of col.batches.slice(from)) if (b.incident && /^player-(failed|stuck)/.test(b.incident.kind)) out.notes.push(`${b.incident.kind}: ${b.incident.reason}`);
  const one = m => ev.find(e => e.m === m);
  const start = one('video.start');
  // ≥ 1080p "on screen": the later of the first frame and the picture's size reaching 1080p (the
  // size is known from the init segment, before a frame shows).
  const hd = one('video.1080')?.v;
  out.first = start?.v; out.hd = hd != null && start ? Math.max(hd, start.v) : hd; out.target = one('video.target')?.v;
  out.start = start && { media_pre: start.media_pre, slow: start.slow, streams_ms: start.streams_ms, start_p: start.start_p, target_p: start.target_p, vision: start.vision, caps_ms: start.caps_ms, mse_open_ms: start.mse_open_ms, gv: start.gv };
  const seeks = ev.filter(e => e.m === 'video.seek').map(e => e.v);
  [out.seek, out.seek_in, out.seek_hov] = seeks;
  const ld = out.pics.find(p => p.ev === 'loadeddata')?.t;
  out.play = out.move_at != null && ld != null ? out.move_at - ld : undefined;
  const sw = ev.filter(e => e.m === 'video.switch');
  out.sw_down = sw.find(e => e.to_p === 720)?.v; out.sw_up = sw.find(e => e.from_p === 720)?.v;
  out.stalls = ev.filter(e => e.m === 'video.stall').length;
  // A picture below 1080p was shown first: ≥ 1080p (the rung's label; Sintel's 1080p is 2048×872)
  // came more than 100 ms after the first frame. Videos without a 1080p rung (the TED talk) never count.
  const ff = out.pics.find(p => p.ev === 'loadeddata')?.t ?? Infinity;
  out.below1080 = out.hd != null && out.first != null && out.hd > out.first + 100;
  out.first_h = out.pics.find(p => p.ev === 'loadeddata')?.h;
  out.session = one('video.session');
  out.slow = out.session?.slow;
  out.stall_ms = out.session?.stall_ms;
  out.throttle = ev.filter(e => e.m === 'video.throttle').map(e => `${e.kind}${e.itag}:${e.why}→${e.step}@${Math.round(e.v)}`);
  out.last_h = out.pics.at(-1)?.h;
  out.min_h = Math.min(...out.pics.filter(p => p.t >= ff && p.h > 0).map(p => p.h));
  return out;
}

(async () => {
  const col = await collector();
  if (THROTTLE) PROXY = await gvProxy(THROTTLE, TMODE);
  const res = [];
  console.log(`builds ${EXTS.map((e, j) => `${LABELS[j]}=${e}`).join(' ')} · ${RUNS} runs × ${VIDEOS.length} · scenarios ${SCEN.join(',')}`);
  for (let r = 0; r < RUNS; r++) {
    for (let i = 0; i < VIDEOS.length; i++) for (let jj = 0; jj < EXTS.length; jj++) {
      const j = (i + r) % 2 ? EXTS.length - 1 - jj : jj;
      EXT = EXTS[j];
      const LABEL = LABELS[j];
      let b;
      try {
        b = await browser(col);
        for (const [k, kind] of SCEN.entries()) {
          const vid = VIDEOS[(i + k) % VIDEOS.length];
          const x = await scenario(b, col, kind, vid).catch(e => ({ kind, vid, notes: ['error ' + e.message.split('\n')[0]] }));
          x.run = r; x.label = LABEL;
          res.push(x);
          if (THROTTLE || HANG) console.log(`${LABEL} ${r}.${i} ${kind.padEnd(5)} ${vid} first ${x.first?.toFixed(0)} hd ${x.hd?.toFixed(0)} stall_ms ${x.stall_ms} h ${x.min_h}..${x.last_h} slow ${JSON.stringify(x.slow)} ${x.throttle?.join(' ')} vision ${JSON.stringify(x.start?.vision)} ${x.notes.join('; ')}`);
          else console.log(`${LABEL} ${r}.${i} ${kind.padEnd(5)} ${vid} first ${x.first?.toFixed(0)} play ${x.play?.toFixed(0)} hd ${x.hd?.toFixed(0)} seek ${x.seek?.toFixed(0)}/${x.seek_in?.toFixed(0)}/${x.seek_hov?.toFixed(0)} stall ${x.stall_ms} sw↓ ${x.sw_down?.toFixed(0)} sw↑ ${x.sw_up?.toFixed(0)} first_h ${x.first_h} streams ${x.start?.streams_ms} vision ${x.start?.vision?.src} gv ${x.start?.gv?.length} ${x.notes.join('; ')}`);
        }
      } catch (e) { console.log('run error', e.message); }
      if (b) { await b.ctx.close().catch(() => {}); fs.rmSync(b.profile, { recursive: true, force: true }); }
    }
  }
  col.srv.close();
  if (PROXY) { PROXY.srv.close(); fs.rmSync(PROXY.dir, { recursive: true, force: true }); console.log('proxy', JSON.stringify(PROXY.stats)); }
  const rows = [];
  for (const LABEL of LABELS) for (const kind of SCEN) {
    const xs = res.filter(x => x.kind === kind && x.label === LABEL);
    const m = f => xs.map(f).filter(Number.isFinite);
    const row = { build: LABEL, scenario: kind, n: xs.length, ok: m(x => x.first).length };
    row.media_pre = xs.filter(x => x.start?.media_pre?.v).length;
    for (const [name, f] of [['first', x => x.first], ['play', x => x.play], ['hd', x => x.hd], ['seek', x => x.seek], ['seek_in', x => x.seek_in], ['seek_hov', x => x.seek_hov], ['sw_down', x => x.sw_down], ['sw_up', x => x.sw_up], ['stall_ms', x => x.stall_ms]]) {
      const a = m(f); row[name] = `${pct(a, 0.5)?.toFixed?.(0)} / ${pct(a, 0.9)?.toFixed?.(0)} (${a.length})`;
    }
    row.below1080 = xs.filter(x => x.below1080).length;
    if (THROTTLE || HANG) {
      row.end_1080 = xs.filter(x => x.last_h >= 1080).length;
      row.hedge_won = xs.reduce((s, x) => s + (x.slow?.hedge_won || 0) + (x.slow?.vision_hedge?.won ? 1 : 0), 0);
      row.no_frame = xs.filter(x => !Number.isFinite(x.first)).length;
    }
    row.stalls = xs.reduce((s, x) => s + (x.stalls || 0), 0);
    rows.push(row);
  }
  console.log(`\n${LABELS.join(' vs ')}${THROTTLE ? ` · googlevideo ${THROTTLE}× slower (${TMODE})` : ''}${HANG ? ' · first VISIONOS request hangs' : ''}: p50 / p90 ms (samples)`);
  console.table(rows);
  if (OUT) fs.writeFileSync(OUT, JSON.stringify({ labels: LABELS, exts: EXTS, runs: RUNS, videos: VIDEOS, results: res, summary: rows }, null, 1));
})().catch(e => { console.error(e); process.exit(1); });
