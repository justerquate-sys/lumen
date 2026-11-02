// A minimal WebSocket server (RFC 6455, no dependencies) standing in for Deepgram's streaming endpoint in the
// e2e suite: it accepts the `token` subprotocol (the key arrives as the second offered protocol), records the
// binary audio and text messages it receives, and lets the test send Results messages. The background worker
// is pointed here with chrome.storage.local `ytcSockTestV1`, so the suite never reaches the real service.
// It also answers the relay's key probe (GET /v1/projects): 200 for the expected key, else 401.
'use strict';
const http = require('node:http'), crypto = require('node:crypto');

function frame(op, payload) {
  const n = payload.length, head = n < 126 ? Buffer.from([0x80 | op, n]) : n < 65536 ? Buffer.from([0x80 | op, 126, n >> 8, n & 255]) : (() => { const b = Buffer.alloc(10); b[0] = 0x80 | op; b[1] = 127; b.writeBigUInt64BE(BigInt(n), 2); return b; })();
  return Buffer.concat([head, payload]);
}

function start({ key }) {
  const conns = [];
  const mock = { conns, key, probes: [], refuse: false, onconn: null, url: '' };
  const server = http.createServer((req, res) => {
    const ok = req.headers.authorization === 'Token ' + mock.key;
    mock.probes.push({ path: req.url, ok });
    res.writeHead(ok ? 200 : 401, { 'content-type': 'application/json', 'access-control-allow-origin': '*', 'access-control-allow-headers': 'authorization' });
    res.end(req.method === 'OPTIONS' ? '' : JSON.stringify(ok ? { projects: [] } : { err_code: 'INVALID_AUTH' }));
  });
  server.on('upgrade', (req, sock) => {
    const protos = String(req.headers['sec-websocket-protocol'] || '').split(',').map(s => s.trim()).filter(Boolean);
    const c = { url: req.url, protos, bins: 0, bytes: 0, texts: [], closed: false, opened: Date.now(), firstBin: 0, sock };
    if (mock.refuse || protos[0] !== 'token' || protos[1] !== mock.key) {
      sock.end('HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n');
      c.refused = true; conns.push(c); return;
    }
    const accept = crypto.createHash('sha1').update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    sock.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\nSec-WebSocket-Protocol: token\r\n\r\n`);
    c.send = obj => { if (!c.closed) sock.write(frame(1, Buffer.from(typeof obj === 'string' ? obj : JSON.stringify(obj)))); };
    c.close = (code = 1000) => { if (c.closed) return; c.closed = true; const p = Buffer.alloc(2); p.writeUInt16BE(code); sock.write(frame(8, p)); sock.end(); };
    let buf = Buffer.alloc(0);
    sock.on('data', d => {
      buf = Buffer.concat([buf, d]);
      for (;;) {
        if (buf.length < 2) return;
        const op = buf[0] & 15, masked = buf[1] & 128;
        let n = buf[1] & 127, at = 2;
        if (n === 126) { if (buf.length < 4) return; n = buf.readUInt16BE(2); at = 4; } else if (n === 127) { if (buf.length < 10) return; n = Number(buf.readBigUInt64BE(2)); at = 10; }
        const mask = masked ? buf.subarray(at, at + 4) : null; at += masked ? 4 : 0;
        if (buf.length < at + n) return;
        const p = Buffer.from(buf.subarray(at, at + n)); buf = buf.subarray(at + n);
        if (mask) for (let i = 0; i < p.length; i++) p[i] ^= mask[i & 3];
        if (op === 2) { c.bins++; c.bytes += p.length; if (!c.firstBin) c.firstBin = Date.now(); }
        else if (op === 1) {
          const t = p.toString('utf8'); c.texts.push(t);
          if (c.ontext) c.ontext(t);
          // Like Deepgram: CloseStream flushes, then the server closes the socket.
          if (t.includes('"CloseStream"')) setTimeout(() => c.close(1000), 15);
        }
        else if (op === 8) { if (!c.closed) { c.closed = true; try { sock.write(frame(8, p.subarray(0, 2))); } catch {} sock.end(); } }
        else if (op === 9) sock.write(frame(10, p));
      }
    });
    sock.on('close', () => { c.closed = true; c.gone = true; });
    sock.on('error', () => {});
    conns.push(c);
    if (mock.onconn) mock.onconn(c);
  });
  return new Promise(r => server.listen(0, '127.0.0.1', () => { mock.url = `http://127.0.0.1:${server.address().port}`; mock.close = () => { for (const c of conns) try { c.sock.destroy(); } catch {} server.close(); }; r(mock); }));
}

// A Deepgram-shaped Results message.
const result = (text, o = {}) => ({ type: 'Results', channel_index: [0, 1], duration: 1, start: 0, is_final: !!o.final, speech_final: !!o.speech, from_finalize: !!o.flush, channel: { alternatives: [{ transcript: text, confidence: 0.97, words: [] }] } });

module.exports = { start, result };
