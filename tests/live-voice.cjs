// A few careful live voice-search runs against the REAL Deepgram streaming API (paid: each run streams a few
// seconds of audio). macOS `say` speaks the phrases into WAV files that Chrome's fake microphone plays once
// (`--use-file-for-fake-audio-capture=…%noloop`); youtube.com documents and InnerTube are stubbed (nothing reaches
// YouTube). Push-to-talk: Option is held from before the speech and released 200 ms after it ends; one hover run
// leaves the mic a second after the speech (the settled transcript searches at once).
// Reports, per run: the transcript searched, and the latency from speech end to the final transcript and from the
// release to the search.
// The key is read from the file named by DEEPGRAM_KEY_FILE into a throwaway profile's chrome.storage only; it is
// never printed. Run: DEEPGRAM_KEY_FILE=<file> node tests/live-voice.cjs
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path'), { execFileSync } = require('node:child_process');
const { chromiumOptions, extensionDir } = require('./browser.cjs');
const EXT = extensionDir();
const KEY = fs.readFileSync(process.env.DEEPGRAM_KEY_FILE, 'utf8').trim();
const LEAD = 1.0, TAIL = 3.0; // seconds of silence before / after the speech in each file

const RUNS = [
  { voice: 'Samantha', text: 'Roblox scripting tutorial for beginners', mode: 'key' },
  { voice: 'Milena', text: 'Как сделать игру в Роблоксе', mode: 'key' },
  { voice: 'Samantha', text: 'Minecraft redstone tutorial', mode: 'hover' },
];

// macOS say → 48 kHz 16-bit mono WAV with LEAD/TAIL silence; returns { file, speech } (speech duration, s).
function speak(dir, i, voice, text) {
  const aiff = path.join(dir, `p${i}.aiff`), raw = path.join(dir, `p${i}-raw.wav`), out = path.join(dir, `p${i}.wav`);
  execFileSync('say', ['-v', voice, '-o', aiff, text]);
  execFileSync('afconvert', ['-f', 'WAVE', '-d', 'LEI16@48000', '-c', '1', aiff, raw]);
  const b = fs.readFileSync(raw);
  let at = 12, data = null;
  while (at + 8 <= b.length) { const id = b.toString('ascii', at, at + 4), n = b.readUInt32LE(at + 4); if (id === 'data') { data = b.subarray(at + 8, at + 8 + n); break; } at += 8 + n + (n & 1); }
  // Trim the synthesizer's own leading/trailing near-silence so the speech end is exact.
  const s16 = i => data.readInt16LE(i * 2), N = data.length / 2, loud = i => Math.abs(s16(i)) > 300;
  let a = 0, z = N - 1; while (a < N && !loud(a)) a++; while (z > a && !loud(z)) z--;
  const speech = data.subarray(a * 2, (z + 1) * 2), sil = n => Buffer.alloc(Math.round(n * 48000) * 2);
  const pcm = Buffer.concat([sil(LEAD), speech, sil(TAIL)]);
  const h = Buffer.alloc(44);
  h.write('RIFF', 0); h.writeUInt32LE(36 + pcm.length, 4); h.write('WAVE', 8); h.write('fmt ', 12); h.writeUInt32LE(16, 16); h.writeUInt16LE(1, 20); h.writeUInt16LE(1, 22);
  h.writeUInt32LE(48000, 24); h.writeUInt32LE(96000, 28); h.writeUInt16LE(2, 32); h.writeUInt16LE(16, 34); h.write('data', 36); h.writeUInt32LE(pcm.length, 40);
  fs.writeFileSync(out, Buffer.concat([h, pcm]));
  return { file: out, speech: speech.length / 2 / 48000 };
}

async function run(r, wav, dir) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-live-voice-'));
  const ctx = await chromium.launchPersistentContext(profile, {
    headless: true, ...chromiumOptions(), viewport: { width: 1400, height: 900 },
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${wav.file}%noloop`],
  });
  const out = { phrase: r.text, mode: r.mode, speech_s: +wav.speech.toFixed(2) };
  try {
    await ctx.route('**/*', route => {
      const req = route.request(), url = new URL(req.url());
      if (url.hostname === 'api.deepgram.com') return route.continue(); // the relay's key probe (service worker)
      if (url.hostname !== 'www.youtube.com') return route.abort();
      if (req.resourceType() === 'document') return route.fulfill({ status: 200, contentType: 'text/html', body: '<!DOCTYPE html><html><body>stub</body></html>' });
      return route.fulfill({ status: 404, contentType: 'application/json', body: '{}' });
    });
    const sw = ctx.serviceWorkers()[0] || await ctx.waitForEvent('serviceworker');
    await sw.evaluate(k => chrome.storage.local.set({ ytcVoiceKeyV1: k }), KEY);
    const page = ctx.pages()[0];
    for (let i = 0; i < 20; i++) {
      await page.goto('https://www.youtube.com/results?search_query=voice', { waitUntil: 'commit' });
      await page.waitForTimeout(300);
      if (await page.locator('#ytc-mic').count()) break;
    }
    await page.waitForTimeout(500);
    // Every transcript change, timed on the page clock (finals are <b>, interim words <i>).
    await page.evaluate(() => { window.__tx = []; new MutationObserver(() => { const q = document.getElementById('ytc-qv'); window.__tx.push([performance.now(), q.querySelector('b')?.textContent || '', q.querySelector('i')?.textContent || '']); }).observe(document.getElementById('ytc-qv'), { childList: true, subtree: true, characterData: true }); });
    const m = await page.locator('#ytc-mic').boundingBox();
    if (r.mode === 'key') await page.keyboard.down('Alt');
    else await page.mouse.move(m.x + m.width - 18, m.y + 18);
    await page.waitForFunction(() => document.documentElement.dataset.ytcVoiceMic, null, { timeout: 5000 });
    const ds = () => page.evaluate(() => ({ ...document.documentElement.dataset }));
    const mic = +(await ds()).ytcVoiceMic;
    // The file starts playing when the capture opens: the speech ends LEAD + speech seconds later.
    const speechEnd = mic + (LEAD + wav.speech) * 1000;
    const wait = speechEnd + (r.mode === 'key' ? 200 : 1000) - (await page.evaluate(() => performance.now()));
    await page.waitForTimeout(Math.max(0, wait));
    if (r.shot) await page.screenshot({ path: r.shot, clip: { x: m.x - 600, y: 0, width: 760, height: 64 } });
    if (r.mode === 'key') await page.keyboard.up('Alt');
    else await page.mouse.move(m.x + 10, m.y + 80);
    await page.waitForFunction(() => document.documentElement.dataset.ytcVoiceSearch || !document.getElementById('ytc-search').dataset.v, null, { timeout: 5000 });
    await page.waitForTimeout(300);
    const d = await ds(), tx = await page.evaluate(() => window.__tx);
    out.searched = new URLSearchParams(new URL(page.url()).search).get('search_query');
    const fin = tx.filter(t => t[1] && !t[2]);
    out.mic_open_ms = +(mic - +d.ytcVoiceStart).toFixed(0);
    out.socket_open_ms = d.ytcVoiceOpen ? +(+d.ytcVoiceOpen - +d.ytcVoiceStart).toFixed(0) : null;
    out.first_words_after_speech_start_ms = tx.length > 1 ? +(tx.find(t => t[1] || t[2])[0] - (mic + LEAD * 1000)).toFixed(0) : null;
    out.speech_end_to_final_ms = fin.length ? +(fin.find(t => t[0] > speechEnd - 300)?.[0] - speechEnd).toFixed(0) : null;
    out.end_to_search_ms = d.ytcVoiceSearch ? +(+d.ytcVoiceSearch - +d.ytcVoiceEnd).toFixed(0) : null;
    out.speech_end_to_search_ms = d.ytcVoiceSearch ? +(+d.ytcVoiceSearch - speechEnd).toFixed(0) : null;
    out.interims = tx.filter(t => t[2]).length;
    if (!d.ytcVoiceSearch) out.note = await page.evaluate(() => (document.getElementById('ytc-vk-m')?.textContent || '') + ' | ' + document.getElementById('ytc-q').value);
  } finally {
    await ctx.close();
    fs.rmSync(profile, { recursive: true, force: true });
  }
  return out;
}

(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-voice-audio-'));
  const shots = process.env.YTC_SHOTS;
  const results = [];
  for (const [i, r] of RUNS.entries()) {
    if (shots) r.shot = path.join(shots, `voice-live-${i}.png`);
    results.push(await run(r, speak(dir, i, r.voice, r.text), dir));
    console.log(JSON.stringify(results.at(-1)));
  }
  fs.rmSync(dir, { recursive: true, force: true });
})().catch(e => { console.error(String(e && e.stack || e).split(KEY).join('<key>')); process.exit(1); });
