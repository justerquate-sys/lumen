// Video starts in the TV app (a debug APK on the emulator or a TV over adb: tv/scripts/run.sh), driven
// through the WebView's DevTools socket like tv/scripts/cdp.cjs. Live network (real youtube.com and
// googlevideo through the app's shell). Per video:
//   cold   the watch URL loaded as a new document          → first frame, ≥ 1080p (or the top rung)
//   focus  a link focused for 1 s (the remote resting on a card: VISIONOS + first media prefetched),
//          then OK (a real Enter key event)                 → first frame, ≥ 1080p
// Times are the page's own (performance.now): first frame = the client's ytcFirstFrame mark.
//   node tools/bench/video-tv.cjs [--runs N] [--videos id,id]
'use strict';
const { execFileSync } = require('node:child_process');
const arg = (n, d) => { const i = process.argv.indexOf('--' + n); return i < 0 ? d : process.argv[i + 1]; };
const RUNS = +arg('runs', 2);
const VIDEOS = arg('videos', 'aqz-KE-bpKQ,dQw4w9WgXcQ,LXb3EKWsInQ,eRsGyueVLvQ').split(',');
const adb = (...a) => execFileSync('adb', a, { encoding: 'utf8' }).trim();
const sleep = ms => new Promise(r => setTimeout(r, ms));
const pct = (a, q) => { const s = a.filter(Number.isFinite).sort((x, y) => x - y); return s.length ? s[Math.min(s.length - 1, Math.floor(q * (s.length - 1) + 0.5))] : NaN; };

// The page's view of the picture (installed on every new document).
const PICS = `window.__p=[];for(const e of ['loadeddata','resize'])document.addEventListener(e,x=>{const v=x.target;if(v&&v.id==='ytc-video')window.__p.push({e,t:performance.now(),h:v.videoHeight})},true);`;

async function connect() {
  const pid = adb('shell', 'pidof', 'app.lumen.tv');
  if (!pid) throw new Error('app not running (tv/scripts/run.sh)');
  adb('forward', 'tcp:9229', `localabstract:webview_devtools_remote_${pid.split(/\s+/)[0]}`);
  const pages = await (await fetch('http://127.0.0.1:9229/json')).json();
  const page = pages.find(p => p.type === 'page' && /youtube\.com/.test(p.url)) || pages.find(p => p.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, no) => { ws.onopen = ok; ws.onerror = no; });
  let n = 0;
  const wait = new Map();
  ws.onmessage = m => { const d = JSON.parse(m.data); if (wait.has(d.id)) { wait.get(d.id)(d); wait.delete(d.id); } };
  const send = (method, params = {}) => new Promise(ok => { const id = ++n; wait.set(id, ok); ws.send(JSON.stringify({ id, method, params })); });
  const ev = async e => { const r = await send('Runtime.evaluate', { expression: e, awaitPromise: true, returnByValue: true }); return r.result?.result?.value; };
  await send('Page.enable');
  await send('Page.addScriptToEvaluateOnNewDocument', { source: PICS });
  return { send, ev, close: () => ws.close() };
}

async function until(c, expr, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) { const v = await c.ev(expr).catch(() => null); if (v) return v; await sleep(100); }
  return null;
}

const result = async c => {
  const p = (await c.ev('JSON.stringify(window.__p||[])').catch(() => '[]')) || '[]';
  return { ff: +(await c.ev('document.documentElement.dataset.ytcFirstFrame')) || NaN, pics: JSON.parse(p) };
};

(async () => {
  const c = await connect();
  const rows = { cold: { first: [], hd: [], h: [] }, focus: { first: [], hd: [], h: [] } };
  for (let r = 0; r < RUNS; r++) for (const v of VIDEOS) {
    // cold: a new document
    await c.ev(`location.href='https://www.youtube.com/watch?v=${v}'`);
    await sleep(500);
    await until(c, `document.documentElement.dataset.ytcFirstFrame && location.search.includes('${v}')`, 30000);
    await sleep(6000);
    let x = await result(c);
    const hd = x.pics.find(p => p.h >= 1080);
    rows.cold.first.push(x.ff); rows.cold.hd.push(hd ? Math.max(hd.t, x.ff) : NaN); rows.cold.h.push(Math.max(0, ...x.pics.map(p => p.h)));
    console.log(`${r} cold  ${v} first ${x.ff.toFixed(0)} hd ${hd ? Math.max(hd.t, x.ff).toFixed(0) : '-'} max ${Math.max(0, ...x.pics.map(p => p.h))}`);
    // focus: from a results page, a link to the next video focused 1 s, then OK
    const next = VIDEOS[(VIDEOS.indexOf(v) + 1) % VIDEOS.length];
    await c.ev(`location.href='https://www.youtube.com/results?search_query=lumen+bench+${r}'`);
    await until(c, `!!document.querySelector('#ytc-list .c a.t')`, 20000);
    await sleep(2000);
    const t0 = await c.ev(`(()=>{const a=document.querySelector('#ytc-list .c a.t');a.setAttribute('href','/watch?v=${next}');a.focus();return performance.now()})()`);
    await sleep(1000);
    const t1 = await c.ev('performance.now()');
    await c.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await c.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await until(c, `document.documentElement.dataset.ytcFirstFrame && +document.documentElement.dataset.ytcFirstFrame > ${t1}`, 30000);
    await sleep(6000);
    x = await result(c);
    const hd2 = x.pics.find(p => p.h >= 1080 && p.t > t1);
    rows.focus.first.push(x.ff - t1); rows.focus.hd.push(hd2 ? Math.max(hd2.t, x.ff) - t1 : NaN); rows.focus.h.push(Math.max(0, ...x.pics.map(p => p.h)));
    console.log(`${r} focus ${next} first ${(x.ff - t1).toFixed(0)} hd ${hd2 ? (Math.max(hd2.t, x.ff) - t1).toFixed(0) : '-'} max ${Math.max(0, ...x.pics.map(p => p.h))} (focused ${(t1 - t0).toFixed(0)} ms before OK)`);
  }
  for (const [k, v] of Object.entries(rows)) console.log(`${k}: first p50 ${pct(v.first, .5)?.toFixed(0)} p90 ${pct(v.first, .9)?.toFixed(0)} · ≥1080p p50 ${pct(v.hd, .5)?.toFixed(0)} (${v.hd.filter(Number.isFinite).length}/${v.hd.length}) · top rung shown ${[...new Set(v.h)].join('/')}`);
  c.close();
})().catch(e => { console.error(e); process.exit(1); });
