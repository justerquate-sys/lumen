// Synthetic live-stream fixtures for the own-player e2e test, made locally (no network),
// shaped like what VISIONOS + googlevideo serve for a live stream (checked 1 Nov 2026):
// an HLS master + media playlist (sequence numbers in the segment URIs) for the timeline,
// and the adaptive URLs (`&sq=N`) answering with self-initializing fragmented MP4:
// ftyp + moov + emsg (YouTube's "Ingestion-Walltime-Us") + moof + mdat.
//
// Chrome's WebCodecs encodes H.264 (two rungs, a keyframe per segment) and AAC-LC
// (48 kHz mono). Segments are 3.2 s: 80 video frames at 25 fps and exactly 150 AAC
// frames, so the content loops (4 segments = 12.8 s) without gaps; a segment's
// `tfdt` is sq × 3.2 s, like a real stream's large media times.
'use strict';
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');

const SEG = 3.2, FPS = 25, LOOP = 4, RATE = 48000, AAC_FRAME = 1024;
const VIDEO = [
  { itag: 134, w: 640, h: 360, bitrate: 500_000, codec: 'avc1.42C01E' },
  { itag: 160, w: 256, h: 144, bitrate: 100_000, codec: 'avc1.42C00C' },
];
const AUDIO = { itag: 140, bitrate: 64_000 };
const DIR = path.join(os.tmpdir(), 'ytc-live-v1');
const VTS = 90000; // video timescale

// ---- ISO BMFF writer
const u8 = n => Buffer.from([n & 255]);
const u16 = n => { const b = Buffer.alloc(2); b.writeUInt16BE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32BE(n >>> 0); return b; };
const u64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return b; };
const box = (t, ...p) => { const body = Buffer.concat(p); return Buffer.concat([u32(body.length + 8), Buffer.from(t), body]); };
const full = (t, v, f, ...p) => box(t, u8(v), Buffer.from([(f >> 16) & 255, (f >> 8) & 255, f & 255]), ...p);
const zeros = n => Buffer.alloc(n);
const matrix = Buffer.concat([u32(0x10000), u32(0), u32(0), u32(0), u32(0x10000), u32(0), u32(0), u32(0), u32(0x40000000)]);

function esds(asc) {
  const desc = (tag, b) => Buffer.concat([u8(tag), u8(b.length), b]);
  const dec = desc(0x04, Buffer.concat([u8(0x40), u8(0x15), Buffer.from([0, 0, 0]), u32(128000), u32(128000), desc(0x05, asc)]));
  return full('esds', 0, 0, desc(0x03, Buffer.concat([u16(1), u8(0), dec, desc(0x06, u8(2))])));
}

function init({ video, w, h, ts, entry }) {
  const mvhd = full('mvhd', 0, 0, u32(0), u32(0), u32(1000), u32(0), u32(0x10000), u16(0x100), zeros(10), matrix, zeros(24), u32(2));
  const tkhd = full('tkhd', 0, 3, u32(0), u32(0), u32(1), u32(0), u32(0), zeros(8), u16(0), u16(0), u16(video ? 0 : 0x100), u16(0), matrix, u32(w << 16), u32(h << 16));
  const mdhd = full('mdhd', 0, 0, u32(0), u32(0), u32(ts), u32(0), u16(0x55c4), u16(0));
  const hdlr = full('hdlr', 0, 0, u32(0), Buffer.from(video ? 'vide' : 'soun'), zeros(12), Buffer.from(video ? 'Video\0' : 'Audio\0'));
  const media = video ? full('vmhd', 0, 1, zeros(8)) : full('smhd', 0, 0, zeros(4));
  const dinf = box('dinf', full('dref', 0, 0, u32(1), full('url ', 0, 1)));
  const stbl = box('stbl', full('stsd', 0, 0, u32(1), entry), full('stts', 0, 0, u32(0)), full('stsc', 0, 0, u32(0)), full('stsz', 0, 0, u32(0), u32(0)), full('stco', 0, 0, u32(0)));
  const trak = box('trak', tkhd, box('mdia', mdhd, hdlr, box('minf', media, dinf, stbl)));
  const mvex = box('mvex', full('trex', 0, 0, u32(1), u32(1), u32(0), u32(0), u32(0)));
  return Buffer.concat([box('ftyp', Buffer.from('dash'), u32(0), Buffer.from('iso6mp41')), box('moov', mvhd, trak, mvex)]);
}

function avc1(w, h, avcC) {
  return box('avc1', zeros(6), u16(1), zeros(16), u16(w), u16(h), u32(0x480000), u32(0x480000), u32(0), u16(1), zeros(32), u16(0x18), u16(0xffff), box('avcC', avcC));
}
function mp4a(asc) {
  return box('mp4a', zeros(6), u16(1), zeros(8), u16(1), u16(16), u16(0), u16(0), u32(RATE << 16), esds(asc));
}

// One media segment: moof (mfhd, traf: tfhd, tfdt, trun) + mdat.
function fragment(seq, base, samples, video) {
  const flags = 0x000701; // data offset, sample duration, size, flags
  const trunLen = 12 + 4 + 4 + samples.length * 12;
  const tfhd = full('tfhd', 0, 0x020000, u32(1));
  const tfdt = full('tfdt', 1, 0, u64(base));
  const mfhd = full('mfhd', 0, 0, u32(seq));
  const moofLen = 8 + mfhd.length + 8 + tfhd.length + tfdt.length + trunLen;
  const rows = samples.map((s, i) => Buffer.concat([u32(s.dur), u32(s.data.length), u32(video ? (i === 0 ? 0x02000000 : 0x01010000) : 0x02000000)]));
  const trun = full('trun', 0, flags, u32(samples.length), u32(moofLen + 8), ...rows);
  const moof = box('moof', mfhd, box('traf', tfhd, tfdt, trun));
  return Buffer.concat([moof, box('mdat', ...samples.map(s => s.data))]);
}

function emsg(sq, wallUs) {
  const msg = `Sequence-Number: ${sq}\r\nIngestion-Walltime-Us: ${wallUs}\r\nTarget-Duration-Us: ${SEG * 1e6}\r\nStreamable: F\r\n\r\n`;
  return full('emsg', 0, 0, Buffer.from('http://youtube.com/streaming/metadata/segment/102015\0\0'), u32(1), u32(0), u32(0), u32(0), Buffer.from(msg));
}

// Runs in the page: encoded chunks per rendition (base64) + codec descriptions.
async function encode({ SEG, FPS, LOOP, VIDEO, AUDIO, RATE }) {
  const b64 = u8 => { let s = ''; for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000)); return btoa(s); };
  const out = {};
  const frames = Math.round(SEG * FPS) * LOOP;
  for (const r of VIDEO) {
    const chunks = []; let desc = null;
    const enc = new VideoEncoder({ output: (c, m) => { if (m?.decoderConfig?.description) desc = b64(new Uint8Array(m.decoderConfig.description)); const d = new Uint8Array(c.byteLength); c.copyTo(d); chunks.push({ k: c.type === 'key', d: b64(d) }); }, error: e => { throw e; } });
    enc.configure({ codec: r.codec, width: r.w, height: r.h, bitrate: r.bitrate, framerate: FPS, latencyMode: 'realtime', avc: { format: 'avc' } });
    const cv = new OffscreenCanvas(r.w, r.h), g = cv.getContext('2d');
    for (let i = 0; i < frames; i++) {
      const t = i / FPS;
      g.fillStyle = `hsl(${(Math.floor(t / SEG) * 80) % 360} 50% 28%)`; g.fillRect(0, 0, r.w, r.h);
      g.fillStyle = '#f33'; g.fillRect((t * r.w / 3) % r.w, r.h * 0.8, r.w / 12, r.h / 12);
      g.fillStyle = '#fff'; g.font = `bold ${Math.round(r.h / 6)}px sans-serif`; g.fillText(`LIVE ${t.toFixed(1)}`, r.w * 0.06, r.h * 0.4);
      g.font = `${Math.round(r.h / 9)}px sans-serif`; g.fillText(`${r.h}p · itag ${r.itag}`, r.w * 0.06, r.h * 0.62);
      const f = new VideoFrame(cv, { timestamp: Math.round(i * 1e6 / FPS), duration: Math.round(1e6 / FPS) });
      enc.encode(f, { keyFrame: i % Math.round(SEG * FPS) === 0 });
      f.close();
      if (enc.encodeQueueSize > 8) await new Promise(res => enc.addEventListener('dequeue', res, { once: true }));
    }
    await enc.flush();
    out[r.itag] = { chunks, desc };
  }
  const chunks = []; let desc = null;
  const ae = new AudioEncoder({ output: (c, m) => { if (m?.decoderConfig?.description) desc = b64(new Uint8Array(m.decoderConfig.description)); const d = new Uint8Array(c.byteLength); c.copyTo(d); chunks.push({ k: true, d: b64(d) }); }, error: e => { throw e; } });
  ae.configure({ codec: 'mp4a.40.2', sampleRate: RATE, numberOfChannels: 1, bitrate: AUDIO.bitrate });
  const total = Math.round(SEG * LOOP * RATE) + 4096;
  for (let i = 0; i < total; i += 960) {
    const data = new Float32Array(960), hz = 330 + 110 * (Math.floor(i / RATE / SEG) % 3);
    for (let j = 0; j < 960; j++) data[j] = 0.08 * Math.sin(2 * Math.PI * hz * (i + j) / RATE);
    const a = new AudioData({ format: 'f32', sampleRate: RATE, numberOfFrames: 960, numberOfChannels: 1, timestamp: Math.round(i * 1e6 / RATE), data });
    ae.encode(a); a.close();
  }
  await ae.flush();
  out[AUDIO.itag] = { chunks, desc };
  return out;
}

/** Ensures the live fixtures exist; returns a `segment(itag, sq, wallMs)` builder. */
async function ensureLive(ctx) {
  const meta = path.join(DIR, 'live.json');
  if (!fs.existsSync(meta)) {
    const page = await ctx.newPage();
    await page.route('https://media-gen.test/', r => r.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html><title>gen</title>' }));
    await page.goto('https://media-gen.test/');
    const enc = await page.evaluate(encode, { SEG, FPS, LOOP, VIDEO, AUDIO, RATE });
    await page.close();
    fs.mkdirSync(DIR, { recursive: true });
    fs.writeFileSync(meta, JSON.stringify(enc));
  }
  const enc = JSON.parse(fs.readFileSync(meta, 'utf8'));
  const per = Math.round(SEG * FPS), aper = Math.round(SEG * RATE / AAC_FRAME);
  const tracks = {};
  for (const r of VIDEO) {
    const chunks = enc[r.itag].chunks.map(c => Buffer.from(c.d, 'base64'));
    tracks[r.itag] = { init: init({ video: true, w: r.w, h: r.h, ts: VTS, entry: avc1(r.w, r.h, Buffer.from(enc[r.itag].desc, 'base64')) }), chunks, video: true };
  }
  const asc = enc[AUDIO.itag].desc ? Buffer.from(enc[AUDIO.itag].desc, 'base64') : Buffer.from([0x11, 0x88]);
  tracks[AUDIO.itag] = { init: init({ video: false, w: 0, h: 0, ts: RATE, entry: mp4a(asc) }), chunks: enc[AUDIO.itag].chunks.map(c => Buffer.from(c.d, 'base64')), video: false };
  const segment = (itag, sq, wallMs) => {
    const t = tracks[itag]; if (!t) return null;
    const c = sq % LOOP;
    const samples = t.video
      ? t.chunks.slice(c * per, (c + 1) * per).map(data => ({ data, dur: VTS / FPS }))
      : t.chunks.slice(c * aper, (c + 1) * aper).map(data => ({ data, dur: AAC_FRAME }));
    const base = t.video ? sq * per * (VTS / FPS) : sq * aper * AAC_FRAME;
    return Buffer.concat([t.init, emsg(sq, Math.round(wallMs * 1000)), fragment(sq, base, samples, t.video)]);
  };
  const formats = [
    ...VIDEO.map(r => ({ itag: r.itag, mimeType: `video/mp4; codecs="${r.codec}"`, bitrate: r.bitrate, width: r.w, height: r.h, fps: FPS, qualityLabel: `${r.h}p`, quality: 'medium', targetDurationSec: SEG, maxDvrDurationSec: 43200 })),
    { itag: AUDIO.itag, mimeType: 'audio/mp4; codecs="mp4a.40.2"', bitrate: AUDIO.bitrate, audioQuality: 'AUDIO_QUALITY_MEDIUM', audioSampleRate: String(RATE), audioChannels: 1, targetDurationSec: SEG, maxDvrDurationSec: 43200 },
  ];
  return { segment, formats, SEG, VIDEO };
}

module.exports = { ensureLive, SEG };

if (require.main === module) {
  const { chromium } = require('playwright');
  (async () => {
    const b = await chromium.launch({ ...require('./browser.cjs').chromiumOptions(), headless: true });
    const ctx = await b.newContext();
    if (process.argv[2] === '--force') fs.rmSync(DIR, { recursive: true, force: true });
    const r = await ensureLive(ctx);
    const s = r.segment(134, 1000, Date.now());
    console.log(DIR, r.formats.map(f => f.itag).join(','), 'segment 134/1000:', s.length, 'bytes');
    await b.close();
  })().catch(e => { console.error(e); process.exit(1); });
}
