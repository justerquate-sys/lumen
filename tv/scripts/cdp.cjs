#!/usr/bin/env node
// Debug builds: evaluates JavaScript in the TV app's page over the WebView's DevTools socket.
//   node tv/scripts/cdp.cjs '<expression>'      (awaits promises; prints the JSON result)
// Forwards localhost:9229 to the app's webview_devtools_remote socket through adb first.
'use strict';
const { execFileSync } = require('node:child_process');
const adb = (...a) => execFileSync('adb', a, { encoding: 'utf8' }).trim();
async function main() {
  const pid = adb('shell', 'pidof', 'app.lumen.tv');
  if (!pid) throw new Error('app not running');
  adb('forward', 'tcp:9229', `localabstract:webview_devtools_remote_${pid.split(/\s+/)[0]}`);
  const pages = await (await fetch('http://127.0.0.1:9229/json')).json();
  const page = pages.find(p => p.type === 'page' && /youtube\.com/.test(p.url)) || pages.find(p => p.type === 'page');
  if (!page) throw new Error('no page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, no) => { ws.onopen = ok; ws.onerror = no; });
  const res = await new Promise(ok => {
    ws.onmessage = m => { const d = JSON.parse(m.data); if (d.id === 1) ok(d); };
    ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression: process.argv[2], awaitPromise: true, returnByValue: true } }));
  });
  ws.close();
  const r = res.result || {};
  if (r.exceptionDetails) { console.error('exception:', JSON.stringify(r.exceptionDetails.exception && r.exceptionDetails.exception.description || r.exceptionDetails)); process.exit(1); }
  console.log(typeof r.result.value === 'string' ? r.result.value : JSON.stringify(r.result.value));
}
main().catch(e => { console.error(String(e.message || e)); process.exit(1); });
