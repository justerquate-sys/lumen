// Records a short real segment set for the own-player e2e test (run once; ~2 googlevideo
// range requests per itag, no InnerTube call). Input: a saved VISIONOS /player response whose
// URLs are still valid (6 h, same IP). Output dir gets visionos.json + itag-<n>.bin, each file
// being bytes 0..end-of-the-last-segment-starting-before SECONDS of that format.
// Usage: node record-media.cjs <player-VISIONOS.json> <outDir> [seconds=60] [itags=278,242,243,249]
'use strict';
const fs = require('node:fs'), path = require('node:path');
const [src, out, secs = '60', list = '278,242,243,249'] = process.argv.slice(2);
if (!src || !out) { console.error('usage: node record-media.cjs <player.json> <outDir> [seconds] [itags]'); process.exit(2); }
const SECONDS = Number(secs);
const pr = JSON.parse(fs.readFileSync(src, 'utf8'));
fs.mkdirSync(out, { recursive: true });
const readUint = (b, p, n) => { let v = 0; for (let i = 0; i < n; i++) v = v * 256 + b[p + i]; return v; };
function vint(b, p, keep) { let len = 1, m = 0x80; while (len <= 8 && !(b[p] & m)) { len++; m >>= 1; }
  let v = keep ? b[p] : b[p] & (m - 1); for (let i = 1; i < len; i++) v = v * 256 + b[p + i]; return [v, len]; }
function cues(b, f) { // starts (byte offsets) and times of every WebM cluster
  let seg = -1, scale = 1e6; const out = [];
  const walk = (from, to) => { for (let q = from; q < to;) {
    const [id, il] = vint(b, q, true), [size, sl] = vint(b, q + il), d = q + il + sl;
    if (id === 0x18538067) { seg = d; walk(d, to); return; }
    if ([0x1549A966, 0x1C53BB6B, 0xBB, 0xB7].includes(id)) walk(d, Math.min(to, d + size));
    else if (id === 0x2AD7B1) scale = readUint(b, d, size);
    else if (id === 0xB3) out.push({ t: readUint(b, d, size) * scale / 1e9 });
    else if (id === 0xF1 && out.length && out[out.length - 1].pos === undefined) out[out.length - 1].pos = seg + readUint(b, d, size);
    q = d + size; } };
  walk(0, b.length);
  return out.map((c, i) => ({ t0: c.t, start: c.pos, end: i + 1 < out.length ? out[i + 1].pos - 1 : Number(f.contentLength) - 1 }));
}
function sidx(b) {
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  for (let p = 0; p + 8 <= b.length;) {
    const size = dv.getUint32(p), type = b.subarray(p + 4, p + 8).toString('latin1');
    if (type === 'sidx') { const v = b[p + 8]; let q = p + 16; const ts = dv.getUint32(q); q += 4; let t, first;
      if (v === 0) { t = dv.getUint32(q); first = dv.getUint32(q + 4); q += 8; } else { t = Number(dv.getBigUint64(q)); first = Number(dv.getBigUint64(q + 8)); q += 16; }
      const n = dv.getUint16(q + 2); q += 4; let off = p + size + first; const segs = [];
      for (let i = 0; i < n; i++, q += 12) { const r = dv.getUint32(q) & 0x7fffffff, d = dv.getUint32(q + 4); segs.push({ t0: t / ts, start: off, end: off + r - 1 }); off += r; t += d; }
      return segs; }
    p += size;
  }
  throw new Error('no sidx');
}
const get = async (url, a, b) => { const r = await fetch(`${url}&range=${a}-${b}`, { headers: { Origin: 'https://www.youtube.com' } });
  if (!r.ok) throw new Error(`${r.status} for ${a}-${b}`); return Buffer.from(await r.arrayBuffer()); };
(async () => {
  for (const itag of list.split(',').map(Number)) {
    const f = pr.streamingData.adaptiveFormats.find(x => x.itag === itag && !x.xtags);
    const head = await get(f.url, 0, Number(f.indexRange.end));
    const segs = f.mimeType.includes('webm') ? cues(head, f) : sidx(head);
    const keep = segs.filter(s => s.t0 < SECONDS);
    const end = keep[keep.length - 1].end;
    const body = await get(f.url, 0, end);
    fs.writeFileSync(path.join(out, `itag-${itag}.bin`), body);
    console.log(itag, f.mimeType, f.qualityLabel || '', 'segments', keep.length, 'bytes', body.length);
    await new Promise(r => setTimeout(r, 400));
  }
  fs.copyFileSync(src, path.join(out, 'visionos.json'));
})().catch(e => { console.error(e); process.exit(1); });
