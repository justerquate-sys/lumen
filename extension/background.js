// Background worker. Generic tooling only: no YouTube logic.
// Public build: the shared tab storage switch and the voice-search socket relay below, nothing else.
// Development build (./build.sh --dev; browser/build.cjs keeps the @dev blocks): also the
// auto-reload and the debug-center delivery.
'use strict';

// ---- Tab-shared memory (chrome.storage.session: never written to disk, cleared when the browser closes). Content
// scripts may use it only after this switch, which the browser keeps for its session; the worker sets it whenever it
// starts (the startup listener makes it start with the browser). The client keeps one thing there: the ready Home
// response (src/ready.rs; CONTRIBUTING "Privacy").
try { chrome.storage.session.setAccessLevel({ accessLevel: 'TRUSTED_AND_UNTRUSTED_CONTEXTS' }).catch(() => {}); } catch {}
chrome.runtime.onStartup.addListener(() => {});


// ---- Socket relay (voice search). A content script asks for a WebSocket over a
// chrome.runtime port: {t:'open', url, protocols, secret, probe}, then {t:'bin', d: base64} audio chunks and
// {t:'text', d} messages (queued until the socket opens). The stored credential named by `secret` is read here and
// appended to the subprotocols, so it never enters the web page; it may only go to its own host (SECRETS), over
// wss/https. When the handshake fails, `probe` is fetched with the credential to tell a rejected key (401/403) from
// a network failure. Back to the page: {t:'open'}, {t:'msg', d: text}, {t:'close', d: code}, {t:'fail', d: reason}.
// Nothing here is logged or reported. ytcSockTestV1 (e2e only): an http://127.0.0.1:port origin that stands in for
// the host, so the test suite never reaches the real service.
const SECRETS = { ytcVoiceKeyV1: { host: 'api.deepgram.com', scheme: 'Token' } }, SOCK_TEST = 'ytcSockTestV1';
const unb64 = s => { const b = atob(s), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; };
chrome.runtime.onConnect.addListener(port => {
  if (port.name !== 'ytc-sock' || !port.sender || !port.sender.tab) return;
  let ws = null, q = [], gone = false, asked = false;
  const post = (t, d) => { if (!gone) try { port.postMessage({ t, d }); } catch {} };
  const end = () => { if (!gone) { gone = true; q = []; try { port.disconnect(); } catch {} } };
  const fail = why => { post('fail', why); end(); };
  const send = x => { if (gone) return; if (ws && ws.readyState === 1) ws.send(x); else if ((!ws || ws.readyState === 0) && q.length < 2000) q.push(x); };
  port.onDisconnect.addListener(() => { gone = true; q = []; if (ws && ws.readyState < 2) ws.close(1000); });
  port.onMessage.addListener(async m => {
    if (!m || gone) return;
    if (m.t === 'bin') return send(unb64(String(m.d)));
    if (m.t === 'text') return send(String(m.d));
    if (m.t !== 'open' || asked) return;
    asked = true;
    const rule = Object.hasOwn(SECRETS, m.secret) ? SECRETS[m.secret] : null;
    let url, probe;
    try { url = new URL(m.url); probe = m.probe ? new URL(m.probe) : null; } catch { return fail('url'); }
    if (!rule || url.protocol !== 'wss:' || url.hostname !== rule.host || (probe && (probe.protocol !== 'https:' || probe.hostname !== rule.host))) return fail('url');
    const got = await chrome.storage.local.get([m.secret, SOCK_TEST]);
    const key = typeof got[m.secret] === 'string' ? got[m.secret] : '';
    if (gone) return;
    if (!key) return fail('nokey');
    if (typeof got[SOCK_TEST] === 'string') {
      const t = new URL(got[SOCK_TEST]);
      if (t.hostname === '127.0.0.1') { url.protocol = 'ws:'; url.host = t.host; if (probe) { probe.protocol = 'http:'; probe.host = t.host; } }
    }
    const protocols = Array.isArray(m.protocols) ? m.protocols.map(String) : [];
    let opened = false;
    try { ws = new WebSocket(url.href, [...protocols, key]); } catch { return fail('key'); } // not a valid subprotocol token
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => { if (gone) { ws.close(1000); return; } opened = true; for (const x of q) ws.send(x); q = []; post('open', ''); };
    ws.onmessage = e => { if (typeof e.data === 'string') post('msg', e.data); };
    ws.onclose = async e => {
      if (opened) { post('close', String(e.code)); end(); return; }
      if (gone) return;
      if (!probe) return fail('net');
      try {
        const r = await fetch(probe.href, { headers: { authorization: rule.scheme + ' ' + key }, credentials: 'omit', cache: 'no-store' });
        fail(r.status === 401 || r.status === 403 ? 'key' : 'net');
      } catch { fail('net'); }
    };
  });
});
