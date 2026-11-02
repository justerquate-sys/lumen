// Synthetic DASH WebM fixtures for the own-player e2e test, made locally (no network):
// Chrome's WebCodecs encodes VP9 renditions (a moving test card with the time and the
// rendition burnt in) and Opus audio; this file muxes them like YouTube's DASH WebM files:
// EBML header, Segment { Info, Tracks | Cues | Clusters }, initRange = up to Tracks,
// indexRange = Cues, video clusters at 5 s keyframes, audio clusters every 10 s.
// Output is cached in os.tmpdir()/ytc-media-v1 (formats.json + itag-<n>.webm).
'use strict';
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');

const SECONDS = 40, FPS = 24, KEY_EVERY = 5 * FPS;
const VIDEO = [
  { itag: 243, w: 640, h: 360, bitrate: 600_000 },
  { itag: 242, w: 428, h: 240, bitrate: 260_000 },
  { itag: 278, w: 256, h: 144, bitrate: 110_000 },
];
const AUDIO = { itag: 249, bitrate: 48_000 };
const DIR = path.join(os.tmpdir(), 'ytc-media-v1');

// ---- EBML writer (8-byte sizes everywhere: simple and like many muxers)
const id = h => Buffer.from(h, 'hex');
const size8 = n => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); b[0] = 0x01; return b; };
const el = (h, ...parts) => { const body = Buffer.concat(parts); return Buffer.concat([id(h), size8(body.length), body]); };
const uint = (h, n, w) => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return Buffer.concat([id(h), Buffer.from([0x80 | w]), b.subarray(8 - w)]); };
const float = (h, x) => { const b = Buffer.alloc(8); b.writeDoubleBE(x); return el(h, b); };
const str = (h, s) => el(h, Buffer.from(s));

function mux({ video, w, h, frames, clusterMs }) {
  const durMs = SECONDS * 1000;
  const ebml = el('1a45dfa3', uint('4286', 1, 1), uint('42f7', 1, 1), uint('42f2', 4, 1), uint('42f3', 8, 1), str('4282', 'webm'), uint('4287', 4, 1), uint('4285', 2, 1));
  const info = el('1549a966', uint('2ad7b1', 1_000_000, 3), float('4489', durMs), str('4d80', 'ytc-fixture'), str('5741', 'ytc-fixture'));
  const opusHead = Buffer.from([...Buffer.from('OpusHead'), 1, 1, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
  const entry = video
    ? el('ae', uint('d7', 1, 1), uint('73c5', 1, 1), uint('83', 1, 1), str('86', 'V_VP9'), el('e0', uint('b0', w, 2), uint('ba', h, 2)))
    : el('ae', uint('d7', 1, 1), uint('73c5', 1, 1), uint('83', 2, 1), str('86', 'A_OPUS'), el('63a2', opusHead), uint('56aa', 6_500_000, 4), uint('56bb', 80_000_000, 4), el('e1', float('b5', 48000), uint('9f', 1, 1)));
  const tracks = el('1654ae6b', entry);
  // Clusters: video at every keyframe, audio every clusterMs.
  const groups = [];
  for (const f of frames) {
    const ms = Math.round(f.t / 1000);
    const start = video ? f.k : !groups.length || ms - groups[groups.length - 1].tc >= clusterMs;
    if (start || !groups.length) groups.push({ tc: ms, blocks: [] });
    const g = groups[groups.length - 1], rel = Buffer.alloc(2);
    rel.writeInt16BE(ms - g.tc);
    g.blocks.push(el('a3', Buffer.from([0x81]), rel, Buffer.from([f.k ? 0x80 : 0]), f.d));
  }
  const clusters = groups.map(g => el('1f43b675', uint('e7', g.tc, 4), ...g.blocks));
  const cuesFor = pos => el('1c53bb6b', ...groups.map((g, i) => el('bb', uint('b3', g.tc, 4), el('b7', uint('f7', 1, 1), uint('f1', pos[i], 8)))));
  const head = info.length + tracks.length + cuesFor(groups.map(() => 0)).length;
  const pos = []; let p = head;
  for (const c of clusters) { pos.push(p); p += c.length; }
  const cues = cuesFor(pos);
  const segment = Buffer.concat([id('18538067'), size8(info.length + tracks.length + cues.length + clusters.reduce((a, c) => a + c.length, 0)), info, tracks, cues, ...clusters]);
  const file = Buffer.concat([ebml, segment]);
  const initEnd = ebml.length + 12 + info.length + tracks.length - 1;
  return { file, initEnd, indexEnd: initEnd + cues.length, clusters: groups.length };
}

// Runs in the page (secure context): returns base64 chunks per rendition.
async function encode({ SECONDS, FPS, KEY_EVERY, VIDEO, AUDIO }) {
  const b64 = u8 => { let s = ''; for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000)); return btoa(s); };
  const out = {};
  for (const r of VIDEO) {
    const chunks = [];
    const enc = new VideoEncoder({ output: c => { const d = new Uint8Array(c.byteLength); c.copyTo(d); chunks.push({ t: c.timestamp, k: c.type === 'key', d: b64(d) }); }, error: e => { throw e; } });
    enc.configure({ codec: 'vp09.00.10.08', width: r.w, height: r.h, bitrate: r.bitrate, framerate: FPS });
    const cv = new OffscreenCanvas(r.w, r.h), g = cv.getContext('2d');
    for (let i = 0; i < SECONDS * FPS; i++) {
      const t = i / FPS, sec = Math.floor(t);
      g.fillStyle = `hsl(${(sec * 37) % 360} 45% 30%)`; g.fillRect(0, 0, r.w, r.h);
      g.fillStyle = '#b7ebca'; g.fillRect((t * r.w / 4) % r.w, r.h * 0.8, r.w / 12, r.h / 12);
      g.fillStyle = '#fff'; g.font = `bold ${Math.round(r.h / 5)}px sans-serif`; g.fillText(`${t.toFixed(1)} s`, r.w * 0.06, r.h * 0.4);
      g.font = `${Math.round(r.h / 9)}px sans-serif`; g.fillText(`${r.h}p · itag ${r.itag}`, r.w * 0.06, r.h * 0.62);
      const f = new VideoFrame(cv, { timestamp: Math.round(i * 1e6 / FPS), duration: Math.round(1e6 / FPS) });
      enc.encode(f, { keyFrame: i % KEY_EVERY === 0 });
      f.close();
      if (enc.encodeQueueSize > 8) await new Promise(res => enc.addEventListener('dequeue', res, { once: true }));
    }
    await enc.flush();
    out[r.itag] = chunks;
  }
  const chunks = [];
  const ae = new AudioEncoder({ output: c => { const d = new Uint8Array(c.byteLength); c.copyTo(d); chunks.push({ t: c.timestamp, k: true, d: b64(d) }); }, error: e => { throw e; } });
  ae.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1, bitrate: AUDIO.bitrate });
  for (let i = 0; i < SECONDS * 50; i++) {
    const data = new Float32Array(960), hz = 220 + 110 * (Math.floor(i / 50) % 4);
    for (let j = 0; j < 960; j++) data[j] = 0.1 * Math.sin(2 * Math.PI * hz * (i * 960 + j) / 48000);
    const a = new AudioData({ format: 'f32', sampleRate: 48000, numberOfFrames: 960, numberOfChannels: 1, timestamp: i * 20000, data });
    ae.encode(a); a.close();
  }
  await ae.flush();
  out[AUDIO.itag] = chunks;
  return out;
}

/** Ensures the fixtures exist (generating them with the given Playwright context). */
async function ensureMedia(ctx) {
  const meta = path.join(DIR, 'formats.json');
  if (fs.existsSync(meta)) return { dir: DIR, formats: JSON.parse(fs.readFileSync(meta, 'utf8')) };
  const page = await ctx.newPage();
  await page.route('https://media-gen.test/', r => r.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html><title>gen</title>' }));
  await page.goto('https://media-gen.test/');
  const enc = await page.evaluate(encode, { SECONDS, FPS, KEY_EVERY, VIDEO, AUDIO });
  await page.close();
  fs.mkdirSync(DIR, { recursive: true });
  const formats = [];
  for (const r of [...VIDEO, AUDIO]) {
    const video = r.itag !== AUDIO.itag;
    const frames = enc[r.itag].map(c => ({ t: c.t, k: c.k, d: Buffer.from(c.d, 'base64') }));
    const m = mux({ video, w: r.w, h: r.h, frames, clusterMs: 10_000 });
    fs.writeFileSync(path.join(DIR, `itag-${r.itag}.webm`), m.file);
    const f = { itag: r.itag, mimeType: video ? 'video/webm; codecs="vp9"' : 'audio/webm; codecs="opus"', bitrate: Math.round(m.file.length * 8 / SECONDS),
      initRange: { start: '0', end: String(m.initEnd) }, indexRange: { start: String(m.initEnd + 1), end: String(m.indexEnd) },
      lastModified: '1700000000000000', contentLength: String(m.file.length), quality: video ? 'medium' : 'tiny', projectionType: 'RECTANGULAR',
      averageBitrate: Math.round(m.file.length * 8 / SECONDS), approxDurationMs: String(SECONDS * 1000) };
    if (video) Object.assign(f, { width: r.w, height: r.h, fps: FPS, qualityLabel: `${r.h}p` });
    else Object.assign(f, { audioQuality: 'AUDIO_QUALITY_LOW', audioSampleRate: '48000', audioChannels: 1 });
    formats.push(f);
    console.log(`  media fixture itag ${r.itag}: ${m.file.length} bytes, ${m.clusters} clusters`);
  }
  fs.writeFileSync(meta, JSON.stringify(formats, null, 1));
  return { dir: DIR, formats };
}

module.exports = { ensureMedia, SECONDS };

if (require.main === module) {
  const { chromium } = require('playwright');
  (async () => {
    const b = await chromium.launch({ ...require('./browser.cjs').chromiumOptions(), headless: true });
    const ctx = await b.newContext();
    if (process.argv[2] === '--force') fs.rmSync(DIR, { recursive: true, force: true });
    const r = await ensureMedia(ctx);
    console.log(r.dir, r.formats.map(f => f.itag).join(','));
    await b.close();
  })().catch(e => { console.error(e); process.exit(1); });
}
