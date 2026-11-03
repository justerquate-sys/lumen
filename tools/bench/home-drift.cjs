// Live measurement: how far YouTube's own Home answer drifts with time when nothing happens in
// between, compared with its reload-to-reload variation. Signed out, YouTube's own page context
// (ytcfg's INNERTUBE_CONTEXT, so the request is the one YouTube's web app sends), no extension.
//
//   node tools/bench/home-drift.cjs [--profiles=2] [--at=0,1,3,5,10,20] (minutes)
//
// Profiles are seeded once (tools/bench/profile.cjs: a signed-out visitor with a little watch history).
// At every time point each profile asks Home twice back to back (a, b):
//   drift(t)     = overlap of the top 24 of a(t) with a(0)
//   variation(t) = overlap of the top 24 of a(t) with b(t)  (YouTube's own reload-to-reload change)
// Talks to the real youtube.com: run by hand, rarely.
'use strict';
const { seeded, launch, homeIds } = require('./profile.cjs');
const arg = (k, d) => { const a = process.argv.find(x => x.startsWith(`--${k}=`)); return a ? a.slice(k.length + 3) : d; };
const P = +arg('profiles', 2), AT = arg('at', '0,1,3,5,10,20').split(',').map(Number);
const TOP = 24;

const overlap = (a, b) => { a = a.slice(0, TOP); b = b.slice(0, TOP); if (!a.length || !b.length) return NaN; const B = new Set(b); return a.filter(x => B.has(x)).length / Math.min(a.length, b.length); };

async function profile(i) {
  const dir = await seeded(`drift${i}`);
  const ctx = await launch(dir, null);
  const page = ctx.pages()[0] || await ctx.newPage();
  // YouTube's own page once (cookies, ytcfg); then no more page loads, only its Home request.
  await page.goto('https://www.youtube.com/', { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.ytcfg && ytcfg.get('INNERTUBE_CONTEXT'), null, { timeout: 30000 });
  await page.evaluate(() => { window.stop(); });
  const first = homeIds(await page.evaluate(() => window.ytInitialData));
  const ask = () => page.evaluate(async () => {
    const r = await fetch('/youtubei/v1/browse?prettyPrint=false', { method: 'POST', credentials: 'include',
      headers: { 'Content-Type': 'application/json', 'X-Youtube-Client-Name': '1', 'X-Youtube-Client-Version': ytcfg.get('INNERTUBE_CLIENT_VERSION'), 'X-Goog-Visitor-Id': ytcfg.get('VISITOR_DATA') },
      body: JSON.stringify({ context: ytcfg.get('INNERTUBE_CONTEXT'), browseId: 'FEwhat_to_watch' }) });
    return r.json();
  }).then(homeIds);
  return { ctx, dir, first, ask };
}

(async () => {
  const ps = [];
  for (let i = 0; i < P; i++) ps.push(await profile(i));
  const t0 = Date.now(), base = [], rows = [];
  for (const m of AT) {
    const wait = t0 + m * 60000 - Date.now();
    if (wait > 0) await new Promise(r => setTimeout(r, wait));
    for (const [i, p] of ps.entries()) {
      const a = await p.ask(), b = await p.ask();
      if (m === AT[0]) base[i] = a;
      const row = { min: m, profile: i, drift: overlap(base[i], a), variation: overlap(a, b), page: overlap(p.first, a), n: a.length };
      rows.push(row);
      console.log(JSON.stringify(row));
    }
  }
  console.log('\nmin  drift(top24 vs t=0)  reload-to-reload variation (overlap a/b)');
  for (const m of AT) {
    const r = rows.filter(x => x.min === m), avg = k => (r.reduce((s, x) => s + x[k], 0) / r.length * 100).toFixed(0);
    console.log(`${String(m).padStart(3)}  ${avg('drift').padStart(4)} % shared       ${avg('variation').padStart(4)} % shared`);
  }
  for (const p of ps) await p.ctx.close();
})().catch(e => { console.error(e); process.exit(1); });
