#!/usr/bin/env node
// Speed bench of the TV app on the attached emulator/TV (debug builds: the WebView's DevTools socket over adb).
// Real YouTube, signed in or out as the app is. Prints one JSON line per measurement and a summary per path.
//   node tv/scripts/bench.cjs [--runs N] [--cpu R] [--query Q] [path ...]
// Paths (default: all): cold (app start → Home painted), reload (Home reload), keys (D-pad press → focus moved and
// painted, on a results grid), hold (a held key: 20 repeats/s), open (card OK → first video frame), seek (←/→ on the
// bar → the frame at the new place, after the 450 ms rest), menu (OK on a player button → its menu painted), back
// (Back on the watch page → the grid painted). --cpu R throttles the page's CPU R× (a slow TV).
'use strict';
const { execFileSync } = require('node:child_process');
const args = process.argv.slice(2), opt = (f, d) => { const i = args.indexOf(f); return i >= 0 ? args[i + 1] : d; };
const RUNS = +opt('--runs', 5), CPU = +opt('--cpu', 1), QUERY = opt('--query', 'lofi hip hop'), CSS = opt('--css', '');
const PATHS = args.filter((a, i) => !a.startsWith('--') && !(i > 0 && args[i - 1].startsWith('--')));
const want = p => !PATHS.length || PATHS.includes(p);
const PKG = 'app.lumen.tv';
const adb = (...a) => execFileSync('adb', a, { encoding: 'utf8' }).trim();
const sleep = ms => new Promise(r => setTimeout(r, ms));
const key = k => adb('shell', 'input', 'keyevent', 'KEYCODE_' + k);
const out = [];
const log = (path, o) => { const r = { path, cpu: CPU, ...o }; out.push(r); console.log(JSON.stringify(r)); };

// ---- a small DevTools client (one page session; reconnects after reloads and app starts)
class Page {
  async connect(timeout = 20000) {
    const end = Date.now() + timeout;
    for (;;) {
      try {
        const pid = adb('shell', 'pidof', PKG).split(/\s+/)[0];
        if (pid) {
          adb('forward', 'tcp:9229', `localabstract:webview_devtools_remote_${pid}`);
          const pages = await (await fetch('http://127.0.0.1:9229/json')).json();
          const p = pages.find(x => x.type === 'page' && /youtube\.com/.test(x.url));
          if (p) {
            this.ws = new WebSocket(p.webSocketDebuggerUrl);
            await new Promise((ok, no) => { this.ws.onopen = ok; this.ws.onerror = no; });
            this.id = 0; this.wait = new Map();
            this.ws.onmessage = m => { const d = JSON.parse(m.data); const w = this.wait.get(d.id); if (w) { this.wait.delete(d.id); w(d); } };
            if (CPU > 1) await this.send('Emulation.setCPUThrottlingRate', { rate: CPU });
            return this;
          }
        }
      } catch {}
      if (Date.now() > end) throw new Error('no page');
      await sleep(40);
    }
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((ok, no) => {
      const t = setTimeout(() => { this.wait.delete(id); no(new Error('timeout ' + method)); }, 30000);
      this.wait.set(id, d => { clearTimeout(t); d.error ? no(new Error(d.error.message)) : ok(d.result); });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expr) {
    const r = await this.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails.exception && r.exceptionDetails.exception.description || r.exceptionDetails));
    return r.result.value;
  }
  async until(expr, timeout = 15000) {
    const end = Date.now() + timeout;
    for (;;) {
      try { const v = await this.eval(expr); if (v) return v; } catch {}
      if (Date.now() > end) throw new Error('timeout: ' + expr);
      await sleep(30);
    }
  }
  close() { try { this.ws.close(); } catch {} }
}

// In-page recorder: per key, input delay (event → first listener), handler (→ last listener), frame (→ the next
// frame after it, rAF + task, like the client's telemetry), whether the focus moved; long frames (LoAF).
const RECORDER = `(() => {
  if (window.__b) return;
  const B = window.__b = { keys: [], loaf: [] };
  addEventListener('keydown', e => { e.__s = performance.now(); e.__a = document.activeElement; }, true);
  addEventListener('keydown', e => {
    const s = e.__s, end = performance.now(), a = e.__a;
    requestAnimationFrame(() => setTimeout(() => B.keys.push({ k: e.key, rep: e.repeat, delay: s - e.timeStamp, handler: end - s, frame: performance.now() - e.timeStamp, moved: document.activeElement !== a, t: e.timeStamp }), 0));
  }, false);
  try { new PerformanceObserver(l => { for (const x of l.getEntries()) B.loaf.push({ t: x.startTime, ms: x.duration, block: x.blockingDuration }); }).observe({ type: 'long-animation-frame' }); } catch {}
})()`;

const pct = (a, p) => { if (!a.length) return null; const s = [...a].sort((x, y) => x - y); return +s[Math.min(s.length - 1, Math.floor(p / 100 * s.length))].toFixed(1); };
const sum = (path, field) => { const v = out.filter(r => r.path === path && typeof r[field] === 'number').map(r => r[field]); return v.length ? { n: v.length, p50: pct(v, 50), p90: pct(v, 90), max: pct(v, 100) } : null; };

async function cold() {
  for (let i = 0; i < RUNS; i++) {
    adb('shell', 'am', 'force-stop', PKG);
    await sleep(1500);
    // (The device's clock just before the start: the page's timeOrigin is on the same clock.)
    const res = adb('shell', `date +%s%3N; am start -W -n ${PKG}/.MainActivity`);
    const t0 = +res.split('\n')[0], total = +(/TotalTime: (\d+)/.exec(res) || [])[1];
    const p = await new Page().connect();
    const m = await p.until(`(() => { const d = document.documentElement.dataset; if (!d.ytcReady) return null; const n = performance.getEntriesByType('navigation')[0] || {}; return { o: performance.timeOrigin, ready: +d.ytcReady, script: +d.ytcScript, wasm: +d.ytcWasm, shell: +d.ytcShell, req: n.requestStart, resp: n.responseStart, src: d.ytcHomeSrc }; })()`, 20000);
    log('cold', { activity_ms: total, nav_start_ms: Math.round(m.o - t0), doc_response_ms: Math.round(m.resp), script_ms: Math.round(m.script), wasm_ms: Math.round(m.wasm), shell_ms: Math.round(m.shell), ready_ms: Math.round(m.ready), total_ms: Math.round(m.o + m.ready - t0), src: m.src });
    p.close();
  }
}

async function reload() {
  let p = await new Page().connect();
  await p.eval(`location.pathname === '/' || location.assign('/')`);
  await sleep(2500);
  for (let i = 0; i < RUNS; i++) {
    p = await new Page().connect();
    await p.until(`!!document.documentElement.dataset.ytcReady`);
    await sleep(2500); // the ready Home response is fetched in idle time after a paint
    const t = await p.eval(`(() => { const t = performance.timeOrigin + performance.now(); setTimeout(() => location.reload(), 0); return t; })()`);
    p.close();
    await sleep(150);
    p = await new Page().connect();
    const m = await p.until(`(() => { const d = document.documentElement.dataset; if (!d.ytcReady || performance.timeOrigin < ${t}) return null; const n = performance.getEntriesByType('navigation')[0] || {}; return { o: performance.timeOrigin, ready: +d.ytcReady, script: +d.ytcScript, resp: n.responseStart, src: d.ytcHomeSrc }; })()`);
    log('reload', { nav_start_ms: Math.round(m.o - t), doc_response_ms: Math.round(m.resp), script_ms: Math.round(m.script), ready_ms: Math.round(m.ready), total_ms: Math.round(m.o + m.ready - t), src: m.src });
  }
  p.close();
}

async function results(p) {
  await p.eval(`location.assign('/results?search_query=' + encodeURIComponent(${JSON.stringify(QUERY)}))`);
  await sleep(300);
  p.close();
  p = await new Page().connect();
  await p.until(`document.querySelectorAll('#ytc-list a.t').length >= 8 && document.activeElement && document.activeElement.matches('#ytc-list a.t')`);
  // (An adopted sheet after the client's own: it wins at equal specificity.)
  if (CSS) await p.eval(`(() => { const st = new CSSStyleSheet(); st.replaceSync(${JSON.stringify(CSS)}); document.adoptedStyleSheets = [...document.adoptedStyleSheets, st]; })()`);
  await sleep(1500);
  await p.eval(RECORDER);
  return p;
}

async function keys() {
  let p = await results(await new Page().connect());
  // Down the grid, right, up, left: single presses, as the remote does them.
  const seq = ['DPAD_DOWN', 'DPAD_DOWN', 'DPAD_RIGHT', 'DPAD_RIGHT', 'DPAD_DOWN', 'DPAD_LEFT', 'DPAD_DOWN', 'DPAD_RIGHT', 'DPAD_UP', 'DPAD_UP', 'DPAD_LEFT', 'DPAD_DOWN', 'DPAD_DOWN', 'DPAD_DOWN', 'DPAD_UP', 'DPAD_RIGHT'];
  for (let r = 0; r < Math.max(1, Math.round(RUNS / 3)); r++) for (const k of seq) { key(k); await sleep(250); }
  await sleep(500);
  const b = await p.eval(`(() => { const b = window.__b; const r = { keys: b.keys, loaf: b.loaf }; b.keys = []; b.loaf = []; return r; })()`);
  for (const k of b.keys) log('keys', { key: k.k, delay_ms: +k.delay.toFixed(1), handler_ms: +k.handler.toFixed(1), frame_ms: +k.frame.toFixed(1), moved: k.moved });
  log('keys-loaf', { count: b.loaf.length, max_ms: Math.round(Math.max(0, ...b.loaf.map(l => l.ms))) });
  p.close();
}

async function hold() {
  let p = await results(await new Page().connect());
  // A held ↓ then →: Android repeats every 50 ms after 400 ms.
  for (const [k, code, n] of [['ArrowDown', 40, 40], ['ArrowRight', 39, 30], ['ArrowUp', 38, 40]]) {
    for (let i = 0; i < n; i++) {
      await p.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: k, code: k, windowsVirtualKeyCode: code, nativeVirtualKeyCode: code, autoRepeat: i > 0 });
      await sleep(i === 0 ? 400 : 50);
    }
    await p.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code: k, windowsVirtualKeyCode: code, nativeVirtualKeyCode: code });
    await sleep(400);
  }
  await sleep(500);
  const b = await p.eval(`(() => { const b = window.__b; const r = { keys: b.keys, loaf: b.loaf }; b.keys = []; b.loaf = []; return r; })()`);
  for (const k of b.keys) log('hold', { key: k.k, rep: k.rep, delay_ms: +k.delay.toFixed(1), handler_ms: +k.handler.toFixed(1), frame_ms: +k.frame.toFixed(1), moved: k.moved });
  log('hold-loaf', { count: b.loaf.length, max_ms: Math.round(Math.max(0, ...b.loaf.map(l => l.ms))), sum_ms: Math.round(b.loaf.reduce((a, l) => a + l.ms, 0)) });
  p.close();
}

// Card OK → first frame, then seek, menu and Back on that watch page.
async function watch() {
  for (let i = 0; i < RUNS; i++) {
    let p = await results(await new Page().connect());
    for (let j = 0; j < i % 4; j++) { key('DPAD_RIGHT'); await sleep(300); }
    await sleep(1200); // the focus rests: its media prefetch (as when the owner looks at a card)
    await p.eval(`window.__b.keys = []`);
    key('ENTER');
    const f = await p.until(`(() => { const d = document.documentElement.dataset; const k = window.__b.keys.find(k => k.k === 'Enter'); return d.ytcFirstFrame && k ? { ff: +d.ytcFirstFrame, t: k.t } : null; })()`, 20000).catch(() => null);
    if (want('open')) log('open', f ? { first_frame_ms: Math.round(f.ff - f.t) } : { failed: true });
    if (!f) { p.close(); continue; }
    await sleep(2500);
    if (want('seek')) {
      // ← (shows the overlay and scrubs), → ×2: one seek when the remote rests.
      await p.eval(`(() => { const v = document.querySelector('#ytc-player video'); window.__seek = null; v.addEventListener('seeked', () => { const t = performance.now(); (v.requestVideoFrameCallback ? v.requestVideoFrameCallback.bind(v) : requestAnimationFrame)(() => { window.__seek = { seeked: t, frame: performance.now() }; }); }, { once: true }); })()`);
      key('DPAD_RIGHT'); await sleep(150); key('DPAD_RIGHT'); await sleep(150); key('DPAD_RIGHT');
      const s = await p.until(`window.__seek && (() => { const ks = window.__b.keys.filter(k => k.k === 'ArrowRight'); const last = ks[ks.length - 1]; return { ...window.__seek, last: last.t }; })()`, 15000).catch(() => null);
      log('seek', s ? { frame_after_rest_ms: Math.round(s.frame - s.last - 450), seeked_after_rest_ms: Math.round(s.seeked - s.last - 450) } : { failed: true });
      await sleep(1500);
    }
    if (want('menu')) {
      // ↓ to the buttons, → to the next one (captions / speed), OK → its menu.
      key('DPAD_DOWN'); await sleep(400); key('DPAD_RIGHT'); await sleep(400);
      await p.eval(`(() => { window.__menu = null; const pl = document.getElementById('ytc-player'); const mo = new MutationObserver(() => { if (pl.classList.contains('menu')) { mo.disconnect(); requestAnimationFrame(() => setTimeout(() => { window.__menu = performance.now(); }, 0)); } }); mo.observe(pl, { attributes: true, attributeFilter: ['class'] }); window.__b.keys = []; })()`);
      key('ENTER');
      const m = await p.until(`window.__menu && (() => { const k = window.__b.keys.find(k => k.k === 'Enter'); return k ? window.__menu - k.t : null; })()`, 8000).catch(() => null);
      log('menu', m ? { menu_ms: Math.round(m) } : { failed: true });
      key('BACK'); await sleep(600);
    }
    if (want('back')) {
      await p.eval(`(() => { window.__back = null; const old = globalThis.YTC_BACK; globalThis.YTC_BACK = () => { const t = performance.now(); const r = old(); if (!r) { const w = () => location.pathname === '/results' && document.querySelector('#ytc-list a.t') ? requestAnimationFrame(() => setTimeout(() => { window.__back = performance.now() - t; }, 0)) : requestAnimationFrame(w); w(); } return r; }; })()`);
      // Back until the watch page is left (the overlay may take one Back first).
      for (let j = 0; j < 3 && !(await p.eval('window.__back')); j++) { key('BACK'); await sleep(900); }
      const b = await p.eval('window.__back');
      log('back', b ? { back_ms: Math.round(b) } : { failed: true });
    }
    p.close();
  }
}

(async () => {
  if (want('cold')) await cold();
  if (want('reload')) await reload();
  if (want('keys')) await keys();
  if (want('hold')) await hold();
  if (['open', 'seek', 'menu', 'back'].some(want)) await watch();
  const s = {
    cold_total: sum('cold', 'total_ms'), cold_nav_start: sum('cold', 'nav_start_ms'), cold_doc_response: sum('cold', 'doc_response_ms'), cold_script: sum('cold', 'script_ms'), cold_ready: sum('cold', 'ready_ms'),
    reload_total: sum('reload', 'total_ms'), reload_doc_response: sum('reload', 'doc_response_ms'),
    key_frame: sum('keys', 'frame_ms'), key_handler: sum('keys', 'handler_ms'), key_delay: sum('keys', 'delay_ms'),
    hold_frame: sum('hold', 'frame_ms'), hold_handler: sum('hold', 'handler_ms'), hold_loaf: out.filter(r => r.path === 'hold-loaf'),
    open_first_frame: sum('open', 'first_frame_ms'), seek: sum('seek', 'frame_after_rest_ms'), menu: sum('menu', 'menu_ms'), back: sum('back', 'back_ms'),
  };
  for (const k of Object.keys(s)) if (s[k] == null || (Array.isArray(s[k]) && !s[k].length)) delete s[k];
  console.log('SUMMARY ' + JSON.stringify(s));
  process.exit(0);
})().catch(e => { console.error(e); process.exit(1); });
