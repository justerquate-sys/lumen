// End-to-end test: loads the real unpacked extension (extension/) in Chromium.
// Every request is answered by context.route (stub youtube.com documents with a
// fake "YouTube app", InnerTube fixtures, stub embed/images); nothing reaches the
// network. Run: npm run test:e2e (or node tests/e2e.cjs).
// Env (optional): LUMEN_EXTENSION_CHROMIUM (a full Chrome/Chromium that loads
// unpacked extensions; default: Playwright's Chromium), YTC_ONLY=<regexp> (run
// matching steps only), YTC_EXT (another extension folder).
// Public build: the debug-center steps that need development-build telemetry are
// skipped, and the public build's own promises are checked instead (no telemetry,
// nothing kept for a collector). Incidents still reach the test through a forwarder
// appended to a temporary copy of the public background worker.
'use strict';
const { chromium } = require('playwright');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path'), zlib = require('node:zlib');
const assert = require('node:assert/strict');
const { ensureMedia, SECONDS } = require('./make-media.cjs');
const { chromiumOptions, extensionDir, isDevBuild } = require('./browser.cjs');
const { ensureLive } = require('./make-live.cjs');
const wsMock = require('./ws-mock.cjs');

// YTC_EXT: another client folder (e.g. an older build) for the Home timing table;
// YTC_TIMING_ONLY=1 skips the checks and prints only that table.
const EXT = extensionDir();
const DEV_BUILD = isDevBuild(EXT);
const TIMING_ONLY = !!process.env.YTC_TIMING_ONLY;
const FIX = path.join(__dirname, 'fixtures');
const fixture = name => zlib.gunzipSync(fs.readFileSync(path.join(FIX, name + '.gz')));
const json = name => JSON.parse(fixture(name).toString('utf8'));
const EVIL = `<img src=x onerror="window.__xss=1">"'&amp; <script>window.__xss=2</script>`;
const GIF = Buffer.from('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7', 'base64');
// Roughly YouTube's real document CSP, including Trusted Types enforcement.
const CSP = "require-trusted-types-for 'script';trusted-types default goog#html polymer-html-literal;script-src 'nonce-n1' 'unsafe-eval' 'strict-dynamic' https: http: 'unsafe-inline';object-src 'none';base-uri 'self'";
// YouTube's own watch page (native mode cleanup): DOM assembled from the old extension's verified fixtures.
const NATIVE_PAGE = fs.readFileSync(path.join(FIX, 'native-watch.html'), 'utf8');
const STUB = `<!DOCTYPE html><html lang="en" style="font-size:10px" dark system-icons><head>
<script nonce="n1">window.__ytAppRan = true;</script>
<link rel="stylesheet" href="/s/desktop/abc123/cssbin/www-main-desktop-watch-page-skeleton.css">
<script nonce="n1" src="/s/desktop/abc123/jsbin/desktop_polymer.vflset/desktop_polymer.js"></script>
</head><body><ytd-app>NATIVE YOUTUBE APP</ytd-app>${'<!-- padding -->'.repeat(2000)}</body></html>`;

function findFirst(o, pred) {
  if (o && typeof o === 'object') {
    if (pred(o)) return o;
    for (const v of Object.values(o)) { const r = findFirst(v, pred); if (r) return r; }
  }
  return null;
}
// Hostile variants of real fixtures for the escaping test.
const search = json('search.json');
const evilSearch = json('search.json');
const vr = findFirst(evilSearch, o => o.videoId && o.longBylineText && !o.navigationEndpoint?.reelWatchEndpoint);
vr.title = { runs: [{ text: EVIL }] };
vr.longBylineText.runs[0].text = EVIL;
const EVIL_ID = vr.videoId;
const next = json('next.json');
const evilNext = json('next.json');
const sec = findFirst(evilNext, o => o.attributedDescription)?.attributedDescription;
sec.content = EVIL + ' bad-link tail';
// "tail" is a real resource, so the hostile line is one the description panel keeps.
sec.commandRuns = [{ startIndex: EVIL.length + 1, length: 8, onTap: { innertubeCommand: { urlEndpoint: { url: 'javascript:window.__xss=3' } } } },
  { startIndex: EVIL.length + 10, length: 4, onTap: { innertubeCommand: { urlEndpoint: { url: 'https://example.com/tail' } } } }];
findFirst(evilNext, o => o.videoPrimaryInfoRenderer).videoPrimaryInfoRenderer.title = { runs: [{ text: EVIL }] };

// A description linking (through YouTube's redirect) to example.net, for the blocked-websites checks (example.com appears in other fixtures).
const linkedNext = json('next.json');
{
  const ad = findFirst(linkedNext, o => o.attributedDescription).attributedDescription;
  ad.content = 'Get the plugin: example.net/ref and more';
  ad.commandRuns = [{ startIndex: 16, length: 15, onTap: { innertubeCommand: { urlEndpoint: { url: 'https://www.youtube.com/redirect?event=video_description&q=https%3A%2F%2Fwww.example.net%2Fref' } } } }];
}
const linkedNextStr = JSON.stringify(linkedNext);
// A description exercising the old extension's description rules: prose, hashtags (text and
// links), a same-video timestamp, a bare link labelled by the line above, another video's link, a blank-line gap.
const descNext = json('next.json');
{
  const ad = findFirst(descNext, o => o.attributedDescription).attributedDescription;
  ad.content = 'Intro prose that should vanish. #intro\nJoin my Discord: Discord #gaming\nProject files:\nDownload\n#tag 0:30 opening\n\nRelated tutorial\nThanks for watching. #thanks';
  const run = (text, cmd, from = 0) => ({ startIndex: ad.content.indexOf(text, from), length: text.length, onTap: { innertubeCommand: cmd } });
  const tag = t => ({ commandMetadata: { webCommandMetadata: { url: '/hashtag/' + t } }, browseEndpoint: { browseId: 'FEhashtag' } });
  ad.commandRuns = [
    run('Discord', { urlEndpoint: { url: 'https://www.youtube.com/redirect?event=video_description&q=https%3A%2F%2Fdiscord.example%2Fjoin' } }, ad.content.indexOf('Discord:') + 8),
    run('#gaming', tag('gaming')),
    run('Download', { urlEndpoint: { url: 'https://files.example/dl' } }),
    run('#tag', tag('tag')),
    run('0:30', { watchEndpoint: { videoId: 'descRULES01', startTimeSeconds: 30 } }),
    run('Related tutorial', { watchEndpoint: { videoId: 'aqz-KE-bpKQ' } }),
  ];
}
const descNextStr = JSON.stringify(descNext);
const noneNext = json('next.json');
findFirst(noneNext, o => o.attributedDescription).attributedDescription.commandRuns = [];
const noneNextStr = JSON.stringify(noneNext);
// Features state for the mocks: Watch later membership, websites checks, Home music/live labels.
// A solid-color 16x9 PNG (late thumbnails in the motion step; the default stub GIF is transparent).
const PNG = (() => {
  const chunk = (type, data) => { const b = Buffer.alloc(12 + data.length); b.writeUInt32BE(data.length, 0); b.write(type, 4, 'ascii'); data.copy(b, 8); b.writeUInt32BE(zlib.crc32(b.subarray(4, 8 + data.length)), 8 + data.length); return b; };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(16, 0); ihdr.writeUInt32BE(9, 4); ihdr[8] = 8; ihdr[9] = 2;
  const row = Buffer.concat([Buffer.from([0]), Buffer.from(Array(16).fill([74, 120, 140]).flat())]);
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(Buffer.concat(Array(9).fill(row)))), chunk('IEND', Buffer.alloc(0))]);
})();
const IMG = { delay: 0, search: 0 }; // motion step: late thumbnails / a slow search (skeleton geometry)
const NET = { down: false, probes: 0 };
// Hedged reads: the first matching InnerTube request never answers (`{ ep, q | v, n }`).
const HEDGE = { stall: null };
const FX = { bot: null, homeChips: false, wlMock: false, wl: new Set(), wlFail: false, linked: new Set(), checkDelay: 0, checksInFlight: 0, checksMax: 0, homeLabels: false, homeShorts: false };
// ---- SponsorBlock (sponsor.ajay.app): the k-anonymity API answers a whole sha256-prefix bucket. Our fixture video
// sits in the real bucket 671e (its other entries, 2 Nov 2026, are the decoys; aqz-KE-bpKQ hashes there too).
const sha4 = id => require('node:crypto').createHash('sha256').update(id).digest('hex').slice(0, 4);
const SB_ID = 'sponsorBB9G', SB_ID2 = 'sponsorBNjX';
const SB = { delay: 0, log: [], videos: { [SB_ID]: [['intro', 0, 2], ['sponsor', 8, 14], ['interaction', 14, 15.5], ['selfpromo', 26, 30], ['poi_highlight', 34, 34]] } };
const SB_DECOYS = json('sponsorblock_671e.json');
async function sponsorBlock(route, req, url) {
  const m = /^\/api\/skipSegments\/([0-9a-f]{4})$/.exec(url.pathname);
  const e = { url: req.url(), headers: req.headers(), sent: Date.now(), prefix: m && m[1] };
  SB.log.push(e);
  if (SB.delay) await new Promise(r => setTimeout(r, SB.delay));
  e.done = Date.now();
  const cors = { 'access-control-allow-origin': '*' };
  const mine = m ? Object.entries(SB.videos).filter(([id]) => sha4(id) === m[1]) : [];
  if (!mine.length) return route.fulfill({ status: 404, headers: cors, contentType: 'text/plain', body: 'Not Found' });
  const seg = ([category, a, b], i) => ({ category, actionType: category === 'poi_highlight' ? 'poi' : 'skip', segment: [a, b], UUID: 'e2e' + i, videoDuration: SECONDS, locked: 0, votes: 1, description: '' });
  return route.fulfill({ status: 200, headers: cors, contentType: 'application/json', body: JSON.stringify([...SB_DECOYS, ...mine.map(([videoID, segs]) => ({ videoID, segments: segs.map(seg) }))]) });
}
// ---- actions (comments, like/subscribe/save/share, notifications, account): stubbed endpoints.
const ACTS = { fail: new Set(), delay: 0, signedIn: false };
const cmTok = (o) => o.continuationItemRenderer?.continuationEndpoint?.continuationCommand?.token;
const T_FIRST = findFirst(next, o => o.sectionIdentifier === 'comment-item-section').contents.map(cmTok)[0];
const sortItems = findFirst(next, o => o.sortFilterSubMenuRenderer).sortFilterSubMenuRenderer.subMenuItems;
const T_TOP = sortItems[0].serviceEndpoint.continuationCommand.token, T_NEW = sortItems[1].serviceEndpoint.continuationCommand.token;
const comments1 = json('comments.json');
const cmItems = comments1.onResponseReceivedEndpoints[1].reloadContinuationItemsCommand.continuationItems;
const T_PAGE2 = cmTok(cmItems[cmItems.length - 1]);
const T_REP = cmTok(cmItems[0].commentThreadRenderer.replies.commentRepliesRenderer.contents[0]);
// A timestamp (0:12 → this video) and hostile text in the second comment.
const ent2 = comments1.frameworkUpdates.entityBatchUpdate.mutations.find(m => m.payload?.commentEntityPayload?.properties.commentId === cmItems[1].commentThreadRenderer.commentViewModel.commentViewModel.commentId).payload.commentEntityPayload;
ent2.properties.content = { content: 'see 0:12 ' + EVIL, commandRuns: [{ startIndex: 4, length: 4, onTap: { innertubeCommand: { watchEndpoint: { videoId: '-Bxc4gJluC0', startTimeSeconds: 12 } } } }] };
const comments1Str = JSON.stringify(comments1);
const newest = json('comments.json');
newest.frameworkUpdates.entityBatchUpdate.mutations.forEach(m => { const c = m.payload?.commentEntityPayload; if (c) c.author.displayName = '@newest-e2e'; });
const newestStr = JSON.stringify(newest);
const comments2 = json('comments2.json');
const c2items = comments2.onResponseReceivedEndpoints[0].appendContinuationItemsAction.continuationItems;
c2items.splice(c2items.length - 1, 1); // last page: no continuation → End
const comments2Str = JSON.stringify(comments2);
const chapters = json('chapters_next.json');
findFirst(chapters, o => o.markersMap).markersMap[0].value.chapters[1].chapterRenderer.timeRangeStartMillis = 20000;
findFirst(chapters, o => o.markersMap).markersMap[0].value.chapters[2].chapterRenderer.timeRangeStartMillis = 30000;
// Descriptions with timecodes (next.json has no chapters of its own): one by YouTube's rules, one not (5 s chapter).
const chapDescNext = content => { const n = json('next.json'); const ad = findFirst(n, o => o.attributedDescription).attributedDescription; ad.content = content; ad.commandRuns = []; return JSON.stringify(n); };
const descChapStr = chapDescNext('Chapters below.\nMore at https://example.com/page?t=1:00\n\n0:00 Intro\n0:12 - Setup\n1. 0:26 Build https://example.com\n\nThanks!');
const descNoChapStr = chapDescNext('0:00 Intro\n0:05 Too short\n0:26 Build');
const chaptersStr = JSON.stringify(chapters);
const SIGNED_TOPBAR = { desktopTopbarRenderer: { topbarButtons: [
  { notificationTopbarButtonRenderer: { notificationCount: 3 } },
  { topbarMenuButtonRenderer: { avatar: { thumbnails: [{ url: 'https://yt3.ggpht.com/e2e-avatar=s88' }] } } }] } };
const notif = (i, extra) => ({ notificationRenderer: { shortMessage: { simpleText: `Notification ${i}` }, sentTimeText: { simpleText: `${i} hours ago` }, read: false,
  thumbnail: { thumbnails: [{ url: 'https://yt3.ggpht.com/n' }] }, navigationEndpoint: { commandMetadata: { webCommandMetadata: { url: '/watch?v=-Bxc4gJluC0' } }, watchEndpoint: { videoId: '-Bxc4gJluC0' } },
  recordClickEndpoint: { recordNotificationInteractionsEndpoint: { serializedInteractionsRequest: `SER${i}` } }, ...extra } });
function actFixture(ep, body) {
  const ok = (o) => ACTS.fail.has(ep) ? { status: 500, body: '{"error":{"code":500,"message":"Backend Error"}}' } : { status: 200, body: JSON.stringify(o) };
  if (ep === 'next' && body.continuation) {
    const t = body.continuation;
    if (t === T_FIRST || t === T_TOP) return { status: 200, body: comments1Str };
    if (t === T_NEW) return { status: 200, body: newestStr };
    if (t === T_PAGE2) return { status: 200, body: comments2Str };
    if (t === T_REP) return { status: 200, body: fixture('replies.json') };
    return null;
  }
  if (ep === 'next' && body.videoId === 'kCc8FmEb1nY') return { status: 200, body: chaptersStr };
  if (ep.startsWith('like/')) return ok({});
  if (ep.startsWith('subscription/')) return ok({ actions: [{ updateSubscribeButtonAction: { subscribed: ep === 'subscription/subscribe' } }] });
  if (ep === 'playlist/get_add_to_playlist') return ok({ contents: [{ addToPlaylistRenderer: { playlists: [
    { playlistAddToOptionRenderer: { playlistId: 'WL', title: { simpleText: 'Watch later' }, privacy: 'PRIVATE', containsSelectedVideos: 'ALL' } },
    { playlistAddToOptionRenderer: { playlistId: 'PLe2e', title: { simpleText: EVIL }, privacy: 'PUBLIC', containsSelectedVideos: 'NONE' } }] } }] });
  if (ep === 'browse/edit_playlist') return ok({ status: 'STATUS_SUCCEEDED' });
  if (ep === 'playlist/create') return ok({ playlistId: 'PLnew123' });
  if (ep === 'notification/get_notification_menu') return ok(body.ctoken
    ? { actions: [{ appendContinuationItemsAction: { continuationItems: [notif(3)] } }] }
    : { actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { sections: [{ multiPageMenuNotificationSectionRenderer: { items: [notif(1, { shortMessage: { simpleText: EVIL } }), notif(2),
        { continuationItemRenderer: { continuationEndpoint: { getNotificationMenuEndpoint: { ctoken: 'CTOK' } } } }] } }] } } } }] });
  if (ep === 'notification/record_interactions') return ok({});
  if (ep === 'account/account_menu') return ok({ actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer: {
    accountName: { simpleText: 'E2E Viewer' }, channelHandle: { simpleText: '@e2e' }, accountPhoto: { thumbnails: [{ url: 'https://yt3.ggpht.com/e2e-avatar=s88' }] } } } } } } }] });
  return null;
}

// Pre-serialized so the route handler adds as little latency as possible.
const searchStr = JSON.stringify(search), evilSearchStr = JSON.stringify(evilSearch), nextStr = JSON.stringify(next), evilNextStr = JSON.stringify(evilNext);
const cache = new Map();
const fx = name => { if (!cache.has(name)) cache.set(name, fixture(name)); return cache.get(name); };
const log = { warm: [], suggest: [], api: [], docs: [], assets: [], external: [], embeds: [], gv: [], stats: [], timedtext: [], live: [], sb: [] };

// ---- own player fixtures: VISIONOS streams (synthetic local WebM), WEB metadata, captions.
let media = null;
const GV = 'https://rr1---sn-fixture.googlevideo.com/videoplayback';
const playerGen = new Map();
const failures = { gone: false };
// Stuck episodes (stalls are not failures): googlevideo requests of these videos are
// parked (no answer) while held; `unstall` answers the parked ones and lets new ones through. `chunks`:
// only media chunks are held (the index answers: data but no picture → the MediaSource is rebuilt).
const STALL = { hold: new Map(), parked: [] };
const unstall = v => { STALL.hold.delete(v); for (const p of STALL.parked.splice(0)) Promise.resolve().then(() => googlevideo(p.route, p.url)).catch(() => {}); };
// YouTube's bot check (2 Nov 2026 wording) and the visitor the visitor_id fixture mints.
const BOT = { responseContext: {}, playabilityStatus: { status: 'LOGIN_REQUIRED', reason: 'Sign in to confirm you’re not a bot' } };
const FRESH_VD = 'CgtGcmVzaEUyRTAwMSjAgICABjIKCgJERRIEEgAgQw%3D%3D';
// FX.bot: 'visitor' = bot check unless the fresh visitor; 'account' = unless signed (SAPISIDHASH); 'all' = always.
const botChecked = (body, headers) => FX.bot === 'visitor' ? body.context.client.visitorData !== FRESH_VD : FX.bot === 'account' ? !headers.authorization : FX.bot === 'all';
function visionFixture(v) {
  if (v === 'unplayable1') return { playabilityStatus: { status: 'UNPLAYABLE', reason: 'Forced failure (e2e)' } };
  if (v === 'members0001') return { playabilityStatus: { status: 'UNPLAYABLE', reason: 'Join this channel to get access to members-only content like this video, and other exclusive perks.' } };
  if (v === 'agerestrict') return { playabilityStatus: { status: 'LOGIN_REQUIRED', reason: 'Sign in to confirm your age' } };
  if (v === 'liveupcomin') return { playabilityStatus: { status: 'LIVE_STREAM_OFFLINE', reason: 'Premieres in 2 hours' }, videoDetails: { videoId: v, isUpcoming: true, isLiveContent: true } };
  if (LIVE_IDS.includes(v)) {
    if (!LIVE.streams.has(v)) LIVE.streams.set(v, { t0: Date.now() });
    const adaptiveFormats = LIVE.fx.formats.map(f => ({ ...f, url: `${GV}?expire=1793600000&ei=E2E&ip=0.0.0.0&id=${v}.2&itag=${f.itag}&source=yt_live_broadcast&live=1&v=${v}&c=VISIONOS` }));
    return { playabilityStatus: { status: 'OK' }, streamingData: { expiresInSeconds: '21540', adaptiveFormats, hlsManifestUrl: `https://manifest.googlevideo.com/api/manifest/hls_variant/expire/1793600000/id/${v}.2/source/yt_live_broadcast/file/index.m3u8`, serverAbrStreamingUrl: 'https://rr1---sn-fixture.googlevideo.com/sabr' },
      videoDetails: { videoId: v, title: 'Fixture live stream', lengthSeconds: '0', isLive: true, isLiveContent: true, author: 'Fixture' } };
  }
  const gen = (playerGen.get(v) || 0) + 1;
  playerGen.set(v, gen);
  const adaptiveFormats = media.formats.map(f => ({ ...f, url: `${GV}?expire=1793600000&ei=E2E&ip=0.0.0.0&id=o-fixture&mn=sn-fixture%2Csn-fixture2&fvip=1&itag=${f.itag}&source=youtube&v=${v}&gen=${gen}&mime=${encodeURIComponent(f.mimeType.split(';')[0])}&clen=${f.contentLength}&c=VISIONOS` }));
  return { responseContext: {}, playabilityStatus: { status: 'OK' }, streamingData: { expiresInSeconds: '21540', adaptiveFormats },
    videoDetails: { videoId: v, title: 'Fixture video', lengthSeconds: String(SECONDS), author: 'Fixture' } };
}
// A Japanese video: ASR + human Japanese tracks, YouTube can translate to English.
const CC_JA = { captionTracks: ['a.ja', '.ja'].map(id => ({ baseUrl: `https://www.youtube.com/api/timedtext?v=ccjapanese1&ei=E2E&lang=ja${id[0] === 'a' ? '&kind=asr' : ''}`,
  name: { simpleText: id[0] === 'a' ? 'Japanese (auto-generated)' : 'Japanese' }, vssId: id, languageCode: 'ja', kind: id[0] === 'a' ? 'asr' : undefined, isTranslatable: true })),
  audioTracks: [{ captionTrackIndices: [0, 1], defaultCaptionTrackIndex: 1 }], defaultAudioTrackIndex: 0,
  translationLanguages: [{ languageCode: 'de', languageName: { simpleText: 'German' } }, { languageCode: 'en', languageName: { simpleText: 'English' } }] };
// Storyboards (the WEB response's seek-preview sprite sheets, real spec format): every VOD fixture but
// descnochap1 has one; frames every 1 s. The sheets are served as SVGs whose cells show their frame number.
const SB_LEVELS = [[48, 27, 100, 10, 10, 0, 'default'], [80, 45, SECONDS, 10, 10, 1000, 'M$M'], [160, 90, SECONDS, 5, 5, 1000, 'M$M'], [320, 180, SECONDS, 3, 3, 1000, 'M$M']];
const sbSpec = v => `https://i.ytimg.com/sb/${v}/storyboard3_L$L/$N.jpg?sqp=E2E|` + SB_LEVELS.map((l, i) => [...l, `rs$E2E${i}`].join('#')).join('|');
function sbSheet(url) {
  const m = /^\/sb\/([\w-]{11})\/storyboard3_L(\d)\/(default|M(\d+))\.jpg$/.exec(url.pathname), L = m && SB_LEVELS[+m[2]];
  if (!L || url.searchParams.get('sigh') !== `rs$E2E${m[2]}` || url.searchParams.get('sqp') !== 'E2E') return null;
  const [w, h, count, cols, rows] = L, per = cols * rows, page = m[4] ? +m[4] : 0, left = Math.min(per, count - page * per);
  const cw = Math.min(left, cols), rh = Math.ceil(left / cols);
  let cells = '';
  for (let k = 0; k < left; k++) {
    const n = page * per + k, x = (k % cols) * w, y = Math.floor(k / cols) * h;
    cells += `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="hsl(${(n * 47) % 360} 45% 32%)"/><rect x="${x + 0.5}" y="${y + 0.5}" width="${w - 1}" height="${h - 1}" fill="none" stroke="#fff" stroke-opacity=".5"/><text x="${x + w / 2}" y="${y + h * 0.62}" font-size="${h * 0.42}" font-family="Arial" font-weight="700" fill="#fff" text-anchor="middle">${n}</text>`;
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${cw * w}" height="${rh * h}">${cells}</svg>`;
}
function webFixture(v) {
  const q = `cl=1&docid=${v}&ei=E2E&fexp=&ns=yt&plid=P&el=detailpage&len=${SECONDS}&of=OF&vm=VM`;
  return { playabilityStatus: { status: 'OK' }, streamingData: { serverAbrStreamingUrl: 'https://rr1---sn-fixture.googlevideo.com/sabr' },
    playbackTracking: { videostatsPlaybackUrl: { baseUrl: `https://s.youtube.com/api/stats/playback?${q}` }, videostatsWatchtimeUrl: { baseUrl: `https://s.youtube.com/api/stats/watchtime?${q}` },
      videostatsScheduledFlushWalltimeSeconds: [10, 20, 30], videostatsDefaultFlushIntervalSeconds: 40 },
    captions: { playerCaptionsTracklistRenderer: v === 'ccjapanese1' ? CC_JA : { captionTracks: [{ baseUrl: `https://www.youtube.com/api/timedtext?v=${v}&ei=E2E&lang=en&e2e=1`, name: { runs: [{ text: 'English' }] }, vssId: '.en', languageCode: 'en' }] } },
    ...(v === 'descnochap1' ? {} : { storyboards: { playerStoryboardSpecRenderer: { spec: sbSpec(v), recommendedLevel: 2 } } }),
    videoDetails: { videoId: v, title: 'Fixture video', lengthSeconds: String(SECONDS) } };
}
const json3 = JSON.stringify({ wireMagic: 'pb3', events: [{ tStartMs: 0, dDurationMs: SECONDS * 1000, id: 1, wpWinPosId: 1, wsWinStyleId: 1 },
  ...Array.from({ length: SECONDS / 2 }, (_, i) => ({ tStartMs: i * 2000, dDurationMs: 2000, wWinId: 1, segs: [{ utf8: `Caption line ${i}` }, { utf8: ` <b>&amp;</b>`, tOffsetMs: 300 }] }))] });
// ---- live fixtures: HLS master/media playlists (sliding window that grows in real
// time, 3.2 s segments) + fMP4 segments by sequence number (make-live.cjs).
const LIVE_IDS = ['livestream1', 'liveended01'];
const LIVE = { fx: null, streams: new Map(), base: 1000, window: 8 };
const liveHead = st => LIVE.base + 5 + Math.floor((Date.now() - st.t0) / (LIVE.fx.SEG * 1000));
function liveManifest(route, url) {
  const p = url.pathname, v = (p.match(/\/id\/([\w-]{11})\.2\//) || [])[1], st = LIVE.streams.get(v);
  log.live.push({ p, v, t: Date.now() });
  const cors = { 'access-control-allow-origin': 'https://www.youtube.com', 'content-type': 'application/vnd.apple.mpegurl' };
  if (!st) return route.fulfill({ status: 404, headers: cors, body: '' });
  if (p.includes('/hls_variant/')) {
    const vs = LIVE.fx.VIDEO.map(r => `#EXT-X-STREAM-INF:BANDWIDTH=${r.bitrate + 70000},CODECS="${r.codec},mp4a.40.2",RESOLUTION=${r.w}x${r.h},FRAME-RATE=25,VIDEO-RANGE=SDR,AUDIO="234",CLOSED-CAPTIONS=NONE\nhttps://manifest.googlevideo.com/api/manifest/hls_playlist/expire/1793600000/id/${v}.2/itag/${r.itag === 134 ? 230 : 229}/source/yt_live_broadcast/playlist_type/DVR/file/index.m3u8`);
    return route.fulfill({ status: 200, headers: cors, body: `#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MEDIA:URI="https://manifest.googlevideo.com/api/manifest/hls_playlist/id/${v}.2/itag/234/file/index.m3u8",TYPE=AUDIO,GROUP-ID="234",NAME="Default",DEFAULT=YES,AUTOSELECT=YES\n${vs.join('\n')}\n` });
  }
  let head = liveHead(st), ended = false;
  if (v === 'liveended01' && head >= LIVE.base + 6) { head = LIVE.base + 6; ended = true; }
  const first = Math.max(LIVE.base, head - LIVE.window + 1);
  let out = `#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:${Math.ceil(LIVE.fx.SEG)}\n#EXT-X-MEDIA-SEQUENCE:${first}\n#EXT-X-DISCONTINUITY-SEQUENCE:0\n`;
  for (let sq = first; sq <= head; sq++) out += `#EXT-X-PROGRAM-DATE-TIME:${new Date(st.t0 + (sq - LIVE.base - 5) * LIVE.fx.SEG * 1000).toISOString()}\n#EXTINF:${LIVE.fx.SEG.toFixed(3)},\nhttps://rr1---sn-fixture.googlevideo.com/videoplayback/id/${v}.2/itag/230/source/yt_live_broadcast/playlist/index.m3u8/sq/${sq}/dur/${LIVE.fx.SEG}/file/seg.ts\n`;
  if (ended) out += '#EXT-X-ENDLIST\n';
  return route.fulfill({ status: 200, headers: cors, body: out });
}
function liveSegment(route, url) {
  const v = url.searchParams.get('v'), itag = +url.searchParams.get('itag'), sq = +url.searchParams.get('sq'), st = LIVE.streams.get(v);
  log.gv.push({ itag: String(itag), v, sq, live: true, t: Date.now() });
  const cors = { 'access-control-allow-origin': 'https://www.youtube.com', 'content-type': 'video/mp4' };
  if (!st || sq > liveHead(st) || sq < LIVE.base) return route.fulfill({ status: 404, headers: cors, body: '' });
  const body = LIVE.fx.segment(itag, sq, st.t0 + (sq - LIVE.base - 5) * LIVE.fx.SEG * 1000);
  return body ? route.fulfill({ status: 200, headers: cors, body }) : route.fulfill({ status: 404, headers: cors, body: '' });
}
const mediaFiles = new Map();
function googlevideo(route, url) {
  if (url.hostname === 'manifest.googlevideo.com') return liveManifest(route, url);
  if (url.searchParams.get('live') === '1') return liveSegment(route, url);
  // The TV app's connection warm-up (through its shell): a tiny request, answered 204.
  if (url.pathname === '/generate_204') { log.warm.push(url.hostname); return route.fulfill({ status: 204, headers: { 'access-control-allow-origin': '*' }, body: '' }); }
  const itag = url.searchParams.get('itag'), v = url.searchParams.get('v'), gen = +url.searchParams.get('gen');
  const [a, b] = (url.searchParams.get('range') || '').split('-').map(Number);
  log.gv.push({ itag, v, gen, a, b, host: url.hostname, t: Date.now() });
  const cors = { 'access-control-allow-origin': 'https://www.youtube.com', 'content-type': 'application/octet-stream' };
  if (STALL.hold.has(v) && (STALL.hold.get(v) !== 'chunks' || a > 0)) return new Promise(() => STALL.parked.push({ route, url }));
  if ((v === 'refresh4033' && gen === 1 && a > 0) || (v === 'gone403gone' && failures.gone)) return route.fulfill({ status: 403, headers: cors, body: '' });
  // Live 1 Nov 2026: high rungs can 302 to plain http, which the browser blocks (status 0).
  if ((v === 'deadupgrade' || v === 'deadstart01') && itag === '243') return route.abort('failed');
  // googlevideo never answers the first player response's URLs (stale cached response / stuck node).
  // A stuck node: never answers on its own host, only on the mirror YouTube lists in mn.
  if (v === 'gvnodehang1' && url.hostname === 'rr1---sn-fixture.googlevideo.com') return new Promise(() => {});
  if (v === 'gvhangfirst' && gen === 1) return new Promise(() => {});
  // A slow node (throttled, not dead): its own host answers only after 4 s; the mirror at once.
  if (v === 'gvslowfirst' && url.hostname === 'rr1---sn-fixture.googlevideo.com') {
    const file = fs.readFileSync(path.join(media.dir, `itag-${itag}.webm`));
    return new Promise(r => setTimeout(r, 4000)).then(() => route.fulfill({ status: 200, headers: cors, body: file.subarray(a, Math.min(b, file.length - 1) + 1) })).catch(() => {});
  }
  // A dropped connection: every request to the video server fails at once (HTTP 0).
  if (v === 'netdown0001' && NET.down) return route.abort('internetdisconnected');
  if (v === 'gvhangcach1' && gen === 1) return new Promise(() => {});
  // A momentary 503 from one node on the starting format's index: asked again, no fallback.
  if (v === 'flaky503vid' && itag === '243' && a === 0 && !failures.flaky503) { failures.flaky503 = true; return route.fulfill({ status: 503, headers: cors, body: '' }); }
  // Chunks of one rung failing mid-video (random redirects): the player steps down.
  if (v === 'stepdown001' && itag === '243' && a > 120000) return route.abort('failed');
  if (!mediaFiles.has(itag)) mediaFiles.set(itag, fs.readFileSync(path.join(media.dir, `itag-${itag}.webm`)));
  const file = mediaFiles.get(itag);
  if (!(a >= 0 && b >= a && a < file.length)) return route.fulfill({ status: 416, headers: cors, body: '' });
  return route.fulfill({ status: 200, headers: cors, body: file.subarray(a, Math.min(b, file.length - 1) + 1) });
}
// ---- signed-in-like Home: real channel lockups as page 1, then five browse
// continuations with fresh video ids (HOMECONT1…5) = 180 videos.
const HOME = { rich: false, delay: 0, latency: 0, stamp: false, served: 0, stall: 0, aborted: 0 };
// Simulated `next` latency (speed table).
const NEXT = { latency: 0 };
const ids11 = str => [...new Set([...str.matchAll(/"contentId":"([A-Za-z0-9_-]{11})"/g)].map(m => m[1]))];
const homeId = (id, n) => id.slice(0, 8) + 'H' + n + 'z';
const homePage = (str, n, next) => {
  for (const id of ids11(str)) str = str.split(id).join(homeId(id, n));
  return str.replace(/"continuationCommand":\{"token":"[^"]+"/g, `"continuationCommand":{"token":"${next}"`);
};
const homeFirst = homePage(fixture('channel_videos.json').toString('utf8'), 0, 'HOMECONT1');
const homeConts = [1, 2, 3, 4, 5].map(n => homePage(fixture('browse_cont.json').toString('utf8'), n, n < 5 ? `HOMECONT${n + 1}` : 'HOMEEND'));
const HOME_ID0 = homeId(ids11(fixture('channel_videos.json').toString('utf8'))[0], 0);
// The same first Home page with an explicit "Music" badge on video 0 and a LIVE badge on video 1.
const homeLabeled = (() => {
  const j = JSON.parse(homeFirst), vids = [];
  (function f(o) { if (o && typeof o === 'object') { if (o.lockupViewModel?.contentType === 'LOCKUP_CONTENT_TYPE_VIDEO') vids.push(o.lockupViewModel); for (const v of Object.values(o)) f(v); } })(j);
  vids[0].metadata.lockupMetadataViewModel.metadata.contentMetadataViewModel.metadataRows[0].badges = [{ badgeViewModel: { badgeText: 'Music' } }];
  const b = vids[1].contentImage.thumbnailViewModel.overlays.find(o => o.thumbnailBottomOverlayViewModel).thumbnailBottomOverlayViewModel.badges[0].thumbnailBadgeViewModel;
  b.text = 'LIVE'; b.badgeStyle = 'THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE';
  return { str: JSON.stringify(j), music: vids[0].contentId, live: vids[1].contentId };
})();
// The same first Home page with a Shorts shelf (the recorded search response's real
// shortsLockupViewModel items, fresh ids) after the first two videos.
const homeShorts = (() => {
  const j = JSON.parse(homeFirst);
  const shelf = findFirst(search, o => o.gridShelfViewModel).gridShelfViewModel.contents;
  let str = JSON.stringify(shelf);
  const ids = [...new Set(shelf.map(it => it.shortsLockupViewModel.onTap.innertubeCommand.reelWatchEndpoint.videoId))];
  const fresh = ids.map((id, i) => id.slice(0, 8) + 'S' + i + 'z');
  ids.forEach((id, i) => { str = str.split(id).join(fresh[i]); });
  const items = JSON.parse(str).map(it => ({ richItemRenderer: { content: it } }));
  findFirst(j, o => o.richGridRenderer).richGridRenderer.contents.splice(2, 0, { richSectionRenderer: { content: { richShelfRenderer: { title: { runs: [{ text: 'Shorts' }] }, contents: items } } } });
  return { str: JSON.stringify(j), ids: fresh, title: shelf[0].shortsLockupViewModel.overlayMetadata.primaryText.content };
})();
// YouTube's Home chip bar (signed-in shape, hand-built: signed-out Home has none) and a chip's feed.
const ncloud = (label, token, sel) => ({ chipCloudChipRenderer: { text: { runs: [{ text: label }] }, isSelected: !!sel, ...(token ? { navigationEndpoint: { continuationCommand: { token } } } : {}) } });
const homeChips = (() => {
  const j = JSON.parse(homeFirst);
  findFirst(j, o => o.richGridRenderer).richGridRenderer.header = { feedFilterChipBarRenderer: { contents: [ncloud('All', null, true), ncloud('Gaming', 'CHIPGAMING'), ncloud('Podcasts', 'CHIPPOD0')] } };
  return JSON.stringify(j);
})();
const chipFeed = (n, sel) => {
  const items = JSON.parse(homePage(fixture('browse_cont.json').toString('utf8'), n, 'HOMEEND')).onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems;
  return JSON.stringify({ onResponseReceivedActions: [
    { reloadContinuationItemsCommand: { slot: 'RELOAD_CONTINUATION_SLOT_HEADER', continuationItems: [{ feedFilterChipBarRenderer: { contents: [ncloud('All', 'CHIPALL0'), ncloud('Gaming', sel === 'g' ? null : 'CHIPGAMING2', sel === 'g'), ncloud('Podcasts', sel === 'p' ? null : 'CHIPPOD2', sel === 'p')] } }] } },
    { reloadContinuationItemsCommand: { slot: 'RELOAD_CONTINUATION_SLOT_BODY', continuationItems: items } }] });
};
// YTC_AUDIT: worst-case data (break-ui) for the layout audit: long and short titles, non-Latin
// scripts, emoji, huge counts, a missing avatar. Served for the query "worst" and the video WORST_ID.
const AUDIT = { worst: false };
const WT = [
  'How to make SMOOTH animations in ROBLOX STUDIO! [Moon Animator Tutorial] — Complete Beginner to Advanced Guide (2026 Edition) with R6 + R15 Rigs, IK, Camera Work & Exporting to Blender',
  '【MV】YOASOBI「アイドル」(Idol) Official Music Video — 4K 60fps 原作: 赤坂アカ × 横槍メンゴ「【推しの子】」',
  'شرح كامل لبرنامج روبلوكس ستوديو للمبتدئين خطوة بخطوة ٢٠٢٦',
  'Hi',
  '🔥🔥 I Built a 1:1 Replica of Tokyo in Roblox Studio (100 Days) 🗼🇯🇵',
  'RobloxStudio_AnimationExport_FINAL_final_v2_rendered_1080p60_noaudio.mp4',
  'Как сделать плавную анимацию в Roblox Studio: полный курс для начинающих',
];
const WC = ['Aleksandra Wiśniewska-Kowalczyk Official Music & Gaming Channel', 'Jo', 'Курцгезагт – В двух словах', 'قناة المبرمج العربي الرسمية', 'MonkeyDev', 'TheVeryLongChannelNameWithoutAnySpacesAtAll2026', 'さくらみこ / Sakura Miko'];
const worstSearchStr = (() => {
  const j = json('search.json'); let i = 0;
  const walk = o => { if (o && typeof o === 'object') { if (o.videoRenderer && i < WT.length) { const v = o.videoRenderer, k = i++; v.title = { runs: [{ text: WT[k] }] }; for (const f of ['longBylineText', 'ownerText', 'shortBylineText']) if (v[f]?.runs?.[0]) v[f].runs[0].text = WC[k]; v.viewCountText = { simpleText: k % 2 ? '1,284,736,912 views' : '1 view' }; v.shortViewCountText = { simpleText: k % 2 ? '1.2B views' : '1 view' }; } for (const x of Object.values(o)) walk(x); } };
  walk(j); return JSON.stringify(j);
})();
const WORST_ID = 'UURYhAVph5g';
const worstNextStr = (() => {
  const j = json('next.json');
  findFirst(j, o => o.videoPrimaryInfoRenderer).videoPrimaryInfoRenderer.title = { runs: [{ text: WT[0] }] };
  const own = findFirst(j, o => o.videoOwnerRenderer).videoOwnerRenderer;
  own.title.runs[0].text = WC[0]; own.subscriberCountText = { simpleText: '312M subscribers' }; delete own.thumbnail;
  let i = 0;
  const walk = o => { if (o && typeof o === 'object') { if (o.lockupViewModel && i < WT.length) { const m = o.lockupViewModel.metadata?.lockupMetadataViewModel; if (m) { const k = i++; m.title.content = WT[(k + 1) % WT.length]; const rows = m.metadata?.contentMetadataViewModel?.metadataRows || []; if (rows[0]?.metadataParts?.[0]) rows[0].metadataParts[0].text.content = WC[k]; if (rows[1]?.metadataParts?.[0]) rows[1].metadataParts[0].text.content = k % 2 ? '1,284,736,912 views' : '1 view'; } } for (const x of Object.values(o)) walk(x); } };
  walk(j);
  return JSON.stringify(j).slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"${WORST_ID}"}}}`;
})();
// Search with YouTube's recommendation shelves (synthetic, built from the recorded search): the recorded
// "People also watched" shelf, plus "For you", "People also search for" and a channel's linked
// "Latest from …" shelf (a real result shelf), and the same shelves in the continuation.
const SHELF_CONT = 'SHELVES-CONTINUATION-TOKEN-0123456789-abcdefghijklmnopqrstuvwxyz';
const SHELF_IDS = { rec: ['shelfrec001', 'shelfrec002', 'shelfrec003'], keep: ['shelfkeep01', 'shelfkeep02'] };
const shelfVr = (id, title) => { const v = JSON.parse(JSON.stringify(findFirst(json('search.json'), o => o.videoId && o.longBylineText && !o.navigationEndpoint?.reelWatchEndpoint))); v.videoId = id; v.title = { runs: [{ text: title }] }; v.navigationEndpoint = { commandMetadata: { webCommandMetadata: { url: '/watch?v=' + id } }, watchEndpoint: { videoId: id } }; delete v.expandableMetadata; return { videoRenderer: v }; };
const shelvesExtra = () => [
  { shelfRenderer: { title: { simpleText: 'For you' }, content: { verticalListRenderer: { items: [shelfVr(SHELF_IDS.rec[0], 'For you pick'), shelfVr(SHELF_IDS.rec[1], 'Another pick')] } } } },
  { horizontalCardListRenderer: { header: { richListHeaderRenderer: { title: { simpleText: 'People also search for' } } }, cards: [{ searchRefinementCardRenderer: { query: { runs: [{ text: 'roblox animation' }] }, searchEndpoint: { searchEndpoint: { query: 'roblox animation' } } } }] } },
  { shelfRenderer: { title: { runs: [{ text: 'Latest from MonkeyDev', navigationEndpoint: { browseEndpoint: { browseId: 'UCmonkeydevchannel000001', canonicalBaseUrl: '/@MonkeyDev' } } }] }, endpoint: { browseEndpoint: { browseId: 'UCmonkeydevchannel000001', canonicalBaseUrl: '/@MonkeyDev' } }, content: { verticalListRenderer: { items: [shelfVr(SHELF_IDS.keep[0], 'Latest one'), shelfVr(SHELF_IDS.keep[1], 'Latest two')] } } } },
];
const shelvesSearchStr = (() => {
  const j = json('search.json');
  const list = j.contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[0].itemSectionRenderer.contents;
  list.splice(3, 0, ...shelvesExtra());
  // Its own continuation token, so the continuation request is recognisable.
  findFirst(j, o => o.continuationItemRenderer).continuationItemRenderer.continuationEndpoint.continuationCommand.token = SHELF_CONT;
  return JSON.stringify(j);
})();
const shelvesContStr = (() => {
  const j = json('search_cont.json');
  const list = j.onResponseReceivedCommands[0].appendContinuationItemsAction.continuationItems[0].itemSectionRenderer.contents;
  list.splice(2, 0, { shelfRenderer: { title: { simpleText: 'People also watched' }, content: { verticalListRenderer: { items: [shelfVr(SHELF_IDS.rec[2], 'Watched pick')] } } } });
  return JSON.stringify(j);
})();
function apiFixture(ep, body) {
  if (AUDIT.worst && ep === 'search' && body.query === 'worst' && !body.continuation && !body.params) return worstSearchStr;
  if (AUDIT.worst && ep === 'next' && body.videoId === WORST_ID && !body.continuation && !body.playlistId) return worstNextStr;
  if (ep === 'browse' && /^CHIP(GAMING|POD)/.test(body.continuation || '')) return body.continuation.startsWith('CHIPGAMING') ? chipFeed(7, 'g') : chipFeed(8, 'p');
  if (ep === 'browse' && body.continuation?.startsWith('HOME')) return homeConts[+body.continuation.slice(8) - 1] || '{"onResponseReceivedActions":[]}';
  if (ep === 'browse' && body.browseId === 'FEwhat_to_watch' && HOME.rich) {
    if (HOME.stamp) { const j = JSON.parse(homeFirst); findFirst(j, o => o.lockupViewModel?.contentType === 'LOCKUP_CONTENT_TYPE_VIDEO').lockupViewModel.metadata.lockupMetadataViewModel.title.content = `Fresh #${++HOME.served}`; return JSON.stringify(j); }
    return FX.homeShorts ? homeShorts.str : FX.homeLabels ? homeLabeled.str : FX.homeChips ? homeChips : homeFirst;
  }
  if (ep === 'playlist/get_add_to_playlist') {
    const v = body.videoIds[0];
    return JSON.stringify({ contents: [{ addToPlaylistRenderer: { playlists: [{ playlistAddToOptionRenderer: { playlistId: 'PLother', containsSelectedVideos: 'NONE' } }, { playlistAddToOptionRenderer: { playlistId: 'WL', containsSelectedVideos: FX.wl.has(v) ? 'ALL' : 'NONE' } }] } }] });
  }
  if (ep === 'browse/edit_playlist') {
    if (FX.wlFail) return { status: 401, body: JSON.stringify({ error: { code: 401, message: 'Request is missing required authentication credential.', status: 'UNAUTHENTICATED' } }) };
    const a = body.actions[0];
    if (a.action === 'ACTION_ADD_VIDEO') FX.wl.add(a.addedVideoId); else FX.wl.delete(a.removedVideoId);
    return JSON.stringify({ status: 'STATUS_SUCCEEDED' });
  }
  // Any filtered search answers with the recorded "This year" response (1 Nov 2026).
  if (ep === 'search' && body.continuation === SHELF_CONT) return shelvesContStr;
  if (ep === 'search' && body.query === 'shelves' && !body.params && !body.continuation) return shelvesSearchStr;
  if (ep === 'search') return body.continuation ? fx('search_cont.json') : body.query === 'xss' ? evilSearchStr : body.params ? fx('search_filtered.json') : searchStr;
  if (ep === 'next') {
    if (body.continuation) return fx('next_cont.json');
    // In a playlist: the recorded list= response (1 Nov 2026), reporting the requested video as current.
    if (body.playlistId) return fx('next_list.json').toString('utf8').slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"${body.videoId}"}}}`;
    if (body.videoId === EVIL_ID) return evilNextStr;
    if (body.videoId === 'descNONE001') return noneNextStr.slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"descNONE001"}}}`;
    if (body.videoId === 'descRULES01') return descNextStr.slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"descRULES01"}}}`;
    if (body.videoId === 'descchaps01' || body.videoId === 'descnochap1') return (body.videoId === 'descchaps01' ? descChapStr : descNoChapStr).slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"${body.videoId}"}}}`;
    if (body.videoId === 'blockLINK01' || FX.linked.has(body.videoId)) return linkedNextStr.slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"${body.videoId}"}}}`;
    // Later duplicate key wins in serde_json: report the requested video as current.
    return nextStr.slice(0, -1) + `,"currentVideoEndpoint":{"watchEndpoint":{"videoId":"${body.videoId}"}}}`;
  }
  if (ep === 'navigation/resolve_url') return fx('resolve.json');
  if (ep === 'player') return JSON.stringify(body.context.client.clientName === 'VISIONOS' ? visionFixture(body.videoId) : webFixture(body.videoId));
  if (ep === 'browse') {
    if (body.continuation) return body.continuation.startsWith('4qmFsgJbEiRWTFBM') ? '{"onResponseReceivedActions":[]}' : fx('browse_cont.json');
    if (body.browseId === 'FEwhat_to_watch') return fx('home.json');
    if (body.browseId?.startsWith('UC')) return fx('channel_videos.json');
    if (body.browseId?.startsWith('VL')) return fx('playlist.json');
  }
  return null;
}

async function handle(route) {
  const req = route.request(), url = new URL(req.url());
  if (url.hostname.endsWith('.googlevideo.com')) return googlevideo(route, url);
  if (url.hostname === '127.0.0.1') return route.continue(); // the test debug-center collector
  if (url.hostname === 'www.youtube.com' && url.pathname === '/generate_204') { NET.probes++; return NET.down ? route.abort('internetdisconnected') : route.fulfill({ status: 204, body: '' }); }
  if (url.hostname === 'sponsor.ajay.app') return sponsorBlock(route, req, url);
  if (url.hostname !== 'www.youtube.com') {
    log.external.push(req.url());
    if (url.hostname === 'i.ytimg.com' && url.pathname.startsWith('/sb/')) {
      log.sb.push({ url: req.url(), t: Date.now() });
      const svg = sbSheet(url);
      return svg ? route.fulfill({ status: 200, contentType: 'image/svg+xml', headers: { 'cache-control': 'public, max-age=7200' }, body: svg }) : route.fulfill({ status: 404, body: '' });
    }
    if (/(^|\.)ytimg\.com$|ggpht\.com$|googleusercontent\.com$/.test(url.hostname)) {
      if (IMG.delay) { await new Promise(r => setTimeout(r, IMG.delay)); return route.fulfill({ status: 200, contentType: 'image/png', headers: { 'cache-control': 'public, max-age=7200' }, body: PNG }); }
      return route.fulfill({ status: 200, contentType: 'image/gif', body: GIF });
    }
    return route.abort();
  }
  const p = url.pathname;
  if (p.startsWith('/youtubei/v1/')) {
    const ep = p.slice('/youtubei/v1/'.length);
    const body = req.postDataJSON() || {};
    log.api.push({ ep, body, headers: req.headers(), t: Date.now(), url: req.url() });
    const hs = HEDGE.stall;
    if (hs && hs.n > 0 && ep === hs.ep && !body.continuation && !url.searchParams.has('fields') && (hs.home ? body.browseId === 'FEwhat_to_watch' : hs.q ? body.query === hs.q : body.videoId === hs.v)) {
      hs.n--;
      await new Promise(r => setTimeout(r, 6000));
      try { await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' }); } catch {}
      return;
    }
    const masked = ep === 'next' && url.searchParams.has('fields') && body.videoId && !body.continuation;
    if (masked && FX.checkDelay) {
      const home = /H\dz$/.test(body.videoId); // Home fixture ids (counted apart from other pages' checks)
      FX.checksInFlight++; FX.checksMax = Math.max(FX.checksMax, FX.checksInFlight);
      if (home) { FX.homeInFlight = (FX.homeInFlight || 0) + 1; FX.homeMax = Math.max(FX.homeMax || 0, FX.homeInFlight); }
      await new Promise(r => setTimeout(r, FX.checkDelay));
      FX.checksInFlight--; if (home) FX.homeInFlight--;
    }
    // The card Watch later checks use their own membership mock (FX.wlMock).
    const act = FX.wlMock && /^(playlist\/get_add_to_playlist|browse\/edit_playlist)$/.test(ep) ? null : actFixture(ep, body);
    if (act) {
      if (ACTS.delay && /^(like|subscription|browse\/edit_playlist)/.test(ep)) await new Promise(r => setTimeout(r, ACTS.delay));
      return route.fulfill({ status: act.status, contentType: 'application/json', body: act.body });
    }
    if (ep === 'search' && IMG.search) await new Promise(r => setTimeout(r, IMG.search));
    if (ep === 'visitor_id') return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ responseContext: { visitorData: FRESH_VD, serviceTrackingParams: [] } }) });
    // A hung player request: the first VISIONOS request for this video never answers (the hedge must).
    if (ep === 'player' && body.videoId === 'vishang0001' && body.context?.client?.clientName === 'VISIONOS' && !FX.visHung) { FX.visHung = true; return new Promise(() => {}); }
    if (ep === 'player' && FX.bot) log.api.at(-1).done = Date.now(); // answered now (the recovery tests order calls by it)
    if (ep === 'player' && FX.bot && body.context?.client?.clientName === 'VISIONOS' && botChecked(body, req.headers())) return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(BOT) });
    let out = apiFixture(ep, body);
    if (out && out.status) return route.fulfill({ status: out.status, contentType: 'application/json', body: out.body });
    if (out && ACTS.signedIn && (ep === 'next' || ep === 'browse') && !body.continuation) { const o = JSON.parse(out); o.topbar = SIGNED_TOPBAR; out = JSON.stringify(o); }
    const home = ep === 'browse' && (body.browseId === 'FEwhat_to_watch' || body.continuation?.startsWith('HOME'));
    // A dropped connection fails Home at once; HOME.stall: the next N first-page requests hang (a hedge test).
    if (home && NET.down) return route.abort('internetdisconnected');
    if (home && !body.continuation && HOME.stall > 0) {
      HOME.stall--;
      await new Promise(r => setTimeout(r, 12000));
      return route.fulfill({ status: 200, contentType: 'application/json', body: out }).catch(() => { HOME.aborted++; });
    }
    if (home && (HOME.latency || (HOME.delay && !body.continuation))) await new Promise(r => setTimeout(r, HOME.latency + (body.continuation ? 0 : HOME.delay)));
    if (ep === 'next' && NEXT.latency && !body.continuation && !masked) await new Promise(r => setTimeout(r, NEXT.latency));
    return out ? route.fulfill({ status: 200, contentType: 'application/json', body: out }) : route.fulfill({ status: 404, body: '{}' });
  }
  if (p.startsWith('/api/stats/')) { log.stats.push(req.url()); return route.fulfill({ status: 204, body: '' }); }
  if (p === '/api/timedtext') { log.timedtext.push(req.url()); return route.fulfill({ status: 200, contentType: 'application/json', body: json3 }); }
  if (p === '/complete/search') {
    log.suggest.push(req.url());
    return route.fulfill({ status: 200, contentType: 'application/json; charset=UTF-8', body: JSON.stringify([url.searchParams.get('q'), [['roblox', 0, [512]], ['roblox <b>evil</b>" x="', 0, [512]]], { k: 1 }]) });
  }
  if (p === '/sw.js_data') { log.api.push({ ep: 'sw.js_data' }); return route.fulfill({ status: 200, contentType: 'application/json', body: fixture('sw_js_data.txt') }); }
  if (p.startsWith('/embed/')) { log.embeds.push(req.url()); return route.fulfill({ status: 200, contentType: 'text/html', body: `<body style="background:#000;color:#fff">EMBED ${p.slice(7)}</body>` }); }
  if (p.startsWith('/s/')) { log.assets.push(req.url()); return route.fulfill({ status: 200, contentType: p.endsWith('.css') ? 'text/css' : 'text/javascript', body: p.endsWith('.css') ? '' : 'window.__nativeBundle=(window.__nativeBundle||0)+1;' }); }
  if (req.resourceType() === 'document') {
    log.docs.push(req.url());
    if (url.searchParams.get('v') === 'ntvfixture1' && url.searchParams.get('yt-native') === '1') return route.fulfill({ status: 200, headers: { 'content-type': 'text/html; charset=utf-8', 'content-security-policy': CSP }, body: NATIVE_PAGE });
    return route.fulfill({ status: 200, headers: { 'content-type': 'text/html; charset=utf-8', 'content-security-policy': CSP }, body: STUB });
  }
  return route.fulfill({ status: 204, body: '' });
}

const results = [];
let dumpDebug = null, holdWatch = false;
// YTC_ONLY=<regex>: run only the matching steps (quicker iteration; some depend on earlier state).
const ONLY = process.env.YTC_ONLY ? new RegExp(process.env.YTC_ONLY) : null;
async function step(name, fn) {
  if (TIMING_ONLY || (ONLY && !ONLY.test(name))) return;
  if (name.startsWith('dev build: ') && !DEV_BUILD) { results.push(['skip', name]); console.log(`skip ${name} (public build)`); return; }
  const t = Date.now();
  try { await fn(); results.push(['ok', name, Date.now() - t]); console.log(`ok   ${name}`); }
  catch (e) {
    results.push(['FAIL', name]); console.log(`FAIL ${name}\n     ${e.stack.split('\n').slice(0, 8).join('\n     ')}`);
    if (/^own (player|live)/.test(name) && dumpDebug) await dumpDebug().catch(() => {});
  }
}
const median = a => { const s = a.filter(x => !Number.isNaN(x)).sort((x, y) => x - y); return s.length ? s[Math.floor(s.length / 2)] : NaN; };

(async () => {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-e2e-'));
  // The public build's worker ignores incident messages; a temporary copy gets a forwarder to the
  // test collector (named by the same storage key the development build uses), so the player and
  // action checks see their incidents in both builds. The copy is otherwise byte-identical.
  let load = EXT;
  if (!DEV_BUILD) {
    load = fs.mkdtempSync(path.join(os.tmpdir(), 'ytc-e2e-ext-'));
    fs.cpSync(EXT, load, { recursive: true, filter: f => !f.includes('_metadata') });
    fs.appendFileSync(path.join(load, 'background.js'), `
// ---- e2e only (appended to a temporary copy of the public build).
chrome.runtime.onMessage.addListener(m => {
  if (!m || (typeof m.ytcIncident !== 'string' && typeof m.ytcPerf !== 'string')) return;
  chrome.storage.local.get('ytcCollectorV1').then(g => g.ytcCollectorV1 && fetch(g.ytcCollectorV1, { method: 'POST', headers: { 'content-type': 'application/json' }, body: m.ytcIncident || m.ytcPerf })).catch(() => {});
});
`);
  }
  console.log(`extension: ${EXT} (${DEV_BUILD ? 'development' : 'public'} build)`);
  const ctx = await chromium.launchPersistentContext(profile, {
    headless: true, ...chromiumOptions(), viewport: { width: 1400, height: 900 },
    args: [`--disable-extensions-except=${load}`, `--load-extension=${load}`, '--autoplay-policy=no-user-gesture-required',
      // Voice search: a fake microphone (a beep), granted without the prompt.
      '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream'],
    // YTC_SCROLLBARS=1: classic always-visible scrollbars (macOS "Show scroll bars: Always"), which take layout width.
    ...(process.env.YTC_SCROLLBARS ? { ignoreDefaultArgs: ['--hide-scrollbars'] } : {}),
  });
  media = await ensureMedia(ctx);
  LIVE.fx = await ensureLive(ctx);
  await ctx.route('**/*', handle);
  // Headless Chromium decodes in software and drops frames under load; the
  // dropped-frame watchdog has its own unit test, so keep it out of e2e timing.
  await ctx.addInitScript(() => { try { localStorage.setItem('ytc-test-nowatchdog', '1'); } catch {} });
  // Debug center: a test collector, so e2e incidents never reach the real one.
  const incidents = [];
  const collector = require('node:http').createServer((req, res) => {
    let body = ''; req.on('data', c => body += c); req.on('end', () => {
      res.writeHead(req.method === 'OPTIONS' ? 204 : 200, { 'access-control-allow-origin': '*', 'access-control-allow-headers': 'content-type', 'access-control-allow-private-network': 'true' });
      if (req.method === 'POST') { try { incidents.push(JSON.parse(body)); } catch {} }
      res.end('ok');
    });
  }).listen(0, '127.0.0.1');
  await new Promise(r => collector.once('listening', r));
  const sw = ctx.serviceWorkers()[0] || await ctx.waitForEvent('serviceworker');
  await sw.evaluate(port => chrome.storage.local.set({ ytcCollectorV1: `http://127.0.0.1:${port}/incident` }), collector.address().port);
  // Voice search: a local stand-in for Deepgram's streaming endpoint (the relay is pointed there), so the
  // suite never reaches the real service.
  const VKEY = 'e2eDeepgramKey0123456789abcdefabcdef0123';
  const dg = await wsMock.start({ key: VKEY });
  await sw.evaluate(u => chrome.storage.local.set({ ytcSockTestV1: u }), dg.url);
  const page = ctx.pages()[0] || await ctx.newPage();
  // The client never leaves for YouTube's page by itself: every native watch
  // document must be one the test opened itself or asked for with the panel's Open on YouTube.
  const nativeOk = [];
  const wrapGoto = p => { const g = p.goto.bind(p); p.goto = (u, o) => { if (String(u).includes('yt-native=1')) nativeOk.push(String(u)); return g(u, o); }; };
  wrapGoto(page); ctx.on('page', wrapGoto);
  // Fetch priorities as Chromium assigns them (background preloads must be Low).
  const prio = [];
  const cdp = await ctx.newCDPSession(page);
  await cdp.send('Network.enable');
  cdp.on('Network.requestWillBeSent', e => { if (e.request.url.includes('/youtubei/v1/browse')) prio.push({ post: e.request.postData || '', p: e.request.initialPriority }); });
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  page.on('console', m => { if (m.type() === 'error' && !/ERR_BLOCKED_BY_CLIENT|Failed to load resource/.test(m.text())) errors.push(m.text()); });
  dumpDebug = async () => {
    console.log('     page: ' + page.url() + ' gv: ' + JSON.stringify(log.gv.slice(-6)));
    const inc = incidents[incidents.length - 1];
    if (inc && inc.player) console.log('     last incident: ' + inc.reason + '\n       ' + inc.player.notes.join('\n       '));
    if (inc && inc.player && process.env.YTC_DUMP) console.log('     PLAYER ' + JSON.stringify(inc.player));
    if (!(await page.locator('#ytc-video').count())) return;
    await page.keyboard.press('Backquote');
    console.log('     debug overlay:\n' + (await page.locator('#ytc-debug').textContent()).split('\n').filter(l => /player|ms  |fallback/.test(l)).join('\n'));
    await page.keyboard.press('Backquote');
  };
  // Incidents since `from`, without the tab's `perf` timing reports: those are sent when a
  // document is left (at most once a minute) and can land during any later step.
  // Speed telemetry (development builds): schema-2 event batches (src/telemetry.rs).
  const telBatches = from => incidents.slice(from).filter(x => x.kind === 'perf' && x.schema === 2);
  const problems = from => incidents.slice(from).filter(x => x.kind !== 'perf').map(x => x.kind + ': ' + x.reason);
  const ds = () => page.evaluate(() => ({ ...document.documentElement.dataset }));
  const apiCount = ep => log.api.filter(a => a.ep === ep && !a.body?.continuation).length;
  // Wait until the extension's content script is registered.
  for (let i = 0; i < 20; i++) {
    await page.goto('https://www.youtube.com/results?search_query=warmup', { waitUntil: 'commit' });
    await page.waitForTimeout(300);
    if (await page.locator('#ytc-top').count()) break;
    await page.waitForTimeout(250);
  }

  await step('search page: app stopped, shell + results from InnerTube', async () => {
    log.api.length = 0;
    await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    assert.equal(await page.evaluate(() => window.__ytAppRan), undefined, 'native inline script ran');
    assert.equal(await page.evaluate(() => window.__nativeBundle), undefined, 'native bundle ran');
    assert.equal(await page.locator('ytd-app').count(), 0);
    assert.equal(await page.evaluate(() => document.documentElement.getAttribute('style')), null, 'YouTube <html> attributes cleared');
    const n = await page.locator('#ytc-list .c').count();
    assert.ok(n >= 12, `cards ${n}`);
    // The recorded "People also watched" shelf is YouTube recommending: none of its videos are results.
    assert.equal(await page.locator('#ytc-list .c[data-v="AH30avEEC9A"], #ytc-list .c[data-v="ILaV0JYHzwY"]').count(), 0, 'People also watched dropped');
    const titles = await page.locator('#ytc-list .c .t').allTextContents();
    assert.ok(titles.includes('Roblox Animation Tutorials'), 'playlist card');
    assert.ok(!titles.some(t => t.includes('How to Make Roblox Animations! #roblox')), 'Short was hidden');
    const s = log.api.find(a => a.ep === 'search');
    assert.equal(s.body.query, 'roblox studio animation');
    assert.equal(s.body.context.client.clientName, 'WEB');
    assert.equal(s.headers['x-youtube-client-name'], '1');
    assert.ok(s.headers['x-youtube-client-version'].startsWith('2.'));
    assert.equal(s.headers['x-origin'], 'https://www.youtube.com');
    assert.equal(s.headers['x-goog-authuser'], '0');
    assert.equal(s.headers.authorization, undefined, 'no SAPISID cookie → no Authorization');
    assert.equal(await page.inputValue('#ytc-q'), 'roblox studio animation');
    assert.ok((await page.title()).includes('roblox studio animation'));
  });

  // Since site bundles are no longer network-blocked (see below), the preload
  // scanner can race window.stop() and download one; what matters is that YouTube's
  // app never executes on a client page.
  await step('YouTube site bundles and inline app never run on client pages', async () => {
    assert.equal(await page.evaluate(() => window.__nativeBundle), undefined, 'site bundle executed');
    assert.equal(await page.evaluate(() => window.__ytAppRan), undefined, 'inline app executed');
    assert.equal(await page.locator('#ytc-top').count(), 1);
  });

  // Site bundles are not network-blocked: YouTube's service worker fetches them
  // outside the page frame, so a block broke the native fallback page (found
  // 1 Nov 2026). window.stop() already keeps them from running on client pages.
  await step('DNR: site bundles, /s/player and /api/stats reachable; telemetry blocked', async () => {
    const r = await page.evaluate(async () => {
      const t = async u => { try { await fetch(u); return 'ok'; } catch { return 'blocked'; } };
      return { desktop: await t('/s/desktop/abc/jsbin/x.js'), underscore: await t('/s/_/ytmainappweb/x.js'), player: await t('/s/player/abc/player_ias.vflset/en_US/base.js'), stats: await t('/api/stats/watchtime?x=1'), log: await t('/youtubei/v1/log_event?alt=json') };
    });
    assert.deepEqual(r, { desktop: 'ok', underscore: 'ok', player: 'ok', stats: 'ok', log: 'blocked' });
  });

  await step('infinite scroll loads a search continuation', async () => {
    // The sentinel is observed with a 1500px margin, so on a tall window the
    // first continuation can start before any scrolling.
    await page.evaluate(() => scrollTo(0, document.body.scrollHeight));
    // (14 cards on the first page now that the recorded "People also watched" shelf is dropped, + 10.)
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length > 20, null, { timeout: 5000 });
    assert.ok(log.api.some(a => a.ep === 'search' && a.body.continuation));
    // The fixture continuation repeats itself: all duplicates → paging stops instead of looping.
    await page.waitForSelector('#ytc-more.done', { state: 'attached', timeout: 5000 });
    assert.ok(log.api.filter(a => a.ep === 'search' && a.body.continuation).length <= 2);
    await page.evaluate(() => scrollTo(0, 0));
  });

  let watchId;
  await step('hover prefetches next; click opens watch via SPA (no document load)', async () => {
    const docs = log.docs.length;
    const card = page.locator('#ytc-list .c').filter({ has: page.locator('a.th[href^="/watch"]') }).nth(1);
    watchId = new URL(await card.locator('a.th').getAttribute('href'), 'https://www.youtube.com').searchParams.get('v');
    await card.locator('a.th').hover();
    await page.waitForFunction(() => true);
    for (let i = 0; i < 20 && !log.api.some(a => a.ep === 'next' && a.body.videoId === watchId); i++) await page.waitForTimeout(25);
    assert.ok(log.api.some(a => a.ep === 'next' && a.body.videoId === watchId), 'prefetch on hover');
    const nexts = apiCount('next');
    await card.locator('a.t').click();
    await page.waitForSelector('#ytc-info h1.wt');
    assert.equal(new URL(page.url()).searchParams.get('v'), watchId);
    assert.equal(log.docs.length, docs, 'no document navigation');
    // (Other cards' next calls may follow: speculative prefetch of the related cards in view.)
    assert.equal(log.api.filter(a => a.ep === 'next' && !a.body.continuation && a.body.videoId === watchId).length, 1, 'prefetched next reused');
    void nexts;
    assert.equal(await page.locator('#ytc-info h1.wt').textContent(), 'How to make SMOOTH animations in ROBLOX STUDIO! [Moon Animator Tutorial]');
    await page.waitForSelector('#ytc-video');
    await page.waitForFunction(() => document.documentElement.dataset.ytcFirstFrame);
    const vis = log.api.filter(a => a.ep === 'player' && a.body.videoId === watchId && a.body.context.client.clientName === 'VISIONOS');
    assert.equal(vis.length, 1, 'VISIONOS prefetched on hover and reused by the click');
    assert.ok(await page.locator('#ytc-list .c').count() >= 20, 'related');
    assert.ok((await page.locator('#ytc-desc').textContent()).includes('MOON ANIMATOR'));
    assert.ok(!(await page.locator('#ytc-desc').textContent()).includes("In today's video"), 'description keeps only resource lines');
    assert.ok(await page.locator('#ytc-desc a[href^="https://create.roblox.com/"]').count() >= 1, 'redirect links unwrapped');
    assert.equal(log.embeds.length, 0, 'no embed');
  });

  await step('channel link uses known browseId (no resolve_url) and renders videos', async () => {
    const resolves = apiCount('navigation/resolve_url');
    await page.click('#ytc-info a.wc');
    await page.waitForSelector('.ch h1');
    assert.equal(new URL(page.url()).pathname, '/@monkeydeveloper');
    assert.equal(await page.locator('.ch h1').textContent(), 'MonkeyDev');
    // 30 from the first response (a continuation may already have appended more).
    assert.ok(await page.locator('#ytc-list .c').count() >= 30);
    assert.equal(apiCount('navigation/resolve_url'), resolves);
    const b = log.api.filter(a => a.ep === 'browse' && !a.body.continuation).pop();
    assert.equal(b.body.browseId, 'UCaZO-66UcqyVT9btiqdDryA');
    assert.equal(b.body.params, 'EgZ2aWRlb3PyBgQKAjoA');
  });

  await step('back/forward restore pages from memory without new requests', async () => {
    await page.waitForTimeout(300); // let the channel's first continuation settle
    // The watch page's player restarts on back/forward (its MediaSource cannot be restored); only page data must come from memory.
    // Speculative next calls for other cards in view are not page requests.
    const pageReqs = () => log.api.filter(a => a.ep !== 'sw.js_data' && a.ep !== 'player' && !a.body?.continuation && (a.ep !== 'next' || a.body.videoId === watchId));
    // Park the pointer on blank header space: hovering links prefetches pages (tested separately).
    await page.mouse.move(2, 2);
    await page.waitForTimeout(150);
    const n = pageReqs().length, docs = log.docs.length;
    await page.evaluate(() => history.back());
    await page.waitForFunction(id => location.search.includes(id) && document.querySelector('#ytc-info h1.wt'), watchId);
    await page.evaluate(() => history.back());
    await page.waitForFunction(() => location.pathname === '/results' && document.querySelectorAll('#ytc-list .c').length > 20);
    assert.equal(await page.inputValue('#ytc-q'), 'roblox studio animation');
    await page.evaluate(() => history.forward());
    await page.waitForFunction(() => document.querySelector('#ytc-info h1.wt'));
    assert.equal(pageReqs().length, n, 'no page requests for back/forward: ' + JSON.stringify(pageReqs().slice(n).map(a => [a.ep, a.body])));
    assert.equal(log.docs.length, docs);
  });

  await step('top-bar search submits through the SPA', async () => {
    const docs = log.docs.length;
    await page.fill('#ytc-q', 'xss');
    await page.press('#ytc-q', 'Enter');
    // Title is set when the results render (the previous watch page's related list also has > 5 cards).
    await page.waitForFunction(() => location.search === '?search_query=xss' && document.title.startsWith('xss') && document.querySelectorAll('#ytc-list .c').length > 5);
    assert.equal(log.docs.length, docs);
  });

  await step('hostile titles/descriptions are escaped', async () => {
    const t = await page.locator(`#ytc-list a.t[href="/watch?v=${EVIL_ID}"]`).textContent();
    assert.equal(t, EVIL);
    assert.equal(await page.locator('#ytc-main img[src="x"]').count(), 0);
    assert.equal(await page.locator('#ytc-main script').count(), 0);
    await page.click(`#ytc-list a.t[href="/watch?v=${EVIL_ID}"]`);
    // The header first shows the remembered card title, then the next response's title.
    await page.waitForFunction(e => document.querySelector('#ytc-info h1.wt')?.textContent === e && document.getElementById('ytc-desc'), EVIL);
    assert.equal(await page.locator('#ytc-info h1.wt').textContent(), EVIL);
    assert.ok((await page.locator('#ytc-desc').textContent()).includes(EVIL));
    assert.equal(await page.locator('#ytc-main a[href^="javascript"]').count(), 0);
    assert.equal(await page.locator('#ytc-main img[src="x"], #ytc-main script').count(), 0);
    assert.equal(await page.evaluate(() => window.__xss), undefined);
    assert.ok((await page.title()).startsWith('<img'), 'title set as text');
  });

  await step('a typing pause fetches the search results, so Enter paints from cache', async () => {
    await page.fill('#ytc-q', '');
    await page.type('#ytc-q', 'roblox warm');
    for (let i = 0; i < 30 && !log.api.some(a => a.ep === 'search' && a.body.query === 'roblox warm'); i++) await page.waitForTimeout(50);
    const before = log.api.filter(a => a.ep === 'search' && a.body.query === 'roblox warm').length;
    assert.equal(before, 1, 'fetched during the pause');
    await page.waitForTimeout(250);
    await page.press('#ytc-q', 'Enter');
    await page.waitForFunction(() => new URLSearchParams(location.search).get('search_query') === 'roblox warm' && document.querySelectorAll('#ytc-list .c').length > 5);
    assert.equal(log.api.filter(a => a.ep === 'search' && a.body.query === 'roblox warm').length, 1, 'Enter used the fetched results');
  });

  await step('direct channel load resolves the handle', async () => {
    const resolves = apiCount('navigation/resolve_url');
    await page.goto('https://www.youtube.com/@monkeydeveloper/videos', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    assert.equal(apiCount('navigation/resolve_url'), resolves + 1);
    assert.equal(log.api.filter(a => a.ep === 'navigation/resolve_url').pop().body.url, 'https://www.youtube.com/@monkeydeveloper');
    // Preloaded items enter the DOM in chunks as the user scrolls.
    await page.waitForFunction(() => { scrollTo(0, document.body.scrollHeight); return document.querySelectorAll('#ytc-list .c').length >= 60; }, null, { timeout: 5000, polling: 100 });
  });

  await step('playlist page', async () => {
    await page.goto('https://www.youtube.com/playlist?list=PL_pGtHsOvhtxX3WHo-Hi268L78XBziem_', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    assert.equal(await page.locator('.feed h1').textContent(), 'Roblox Animation Tutorials');
    assert.equal(await page.locator('#ytc-list .c').count(), 6);
    // (Home/Subscriptions may refresh in the background meanwhile.)
    assert.equal(log.api.filter(a => a.ep === 'browse' && !a.body.continuation && !/^FE(what_to_watch|subscriptions)$/.test(a.body.browseId)).pop().body.browseId, 'VLPL_pGtHsOvhtxX3WHo-Hi268L78XBziem_');
  });

  await step('signed-out Home shows the search-first page; WL asks to sign in', async () => {
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('.hero');
    assert.ok((await page.locator('.hero').textContent()).includes('Your YouTube history is off'));
    assert.equal(await page.getAttribute('#ytc', 'data-nav'), 'home');
    const n = log.api.length;
    await page.click('nav a[data-nav=wl]');
    await page.waitForSelector('.msg');
    assert.ok((await page.locator('.msg h1').textContent()).includes('Sign in'));
    assert.equal(log.api.length, n, 'no request without a session');
  });

  await step('Home preloads 100 videos in the background (Low priority); the DOM gets them in chunks as you scroll', async () => {
    HOME.rich = true;
    await page.evaluate(() => sessionStorage.clear());
    const before = log.api.length; prio.length = 0;
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    await page.waitForTimeout(300);
    const shown = await page.locator('#ytc-list .c').count();
    assert.ok(shown >= 30 && shown < 100, `cards in DOM ${shown}: preloaded items wait for scrolling`);
    const conts = log.api.slice(before).filter(a => a.body?.continuation?.startsWith('HOME'));
    assert.ok(conts.length >= 3 && conts.length <= 4, `continuations ${conts.length}`);
    const bg = prio.filter(x => x.post.includes('HOMECONT'));
    assert.ok(bg.filter(x => x.p === 'Low').length >= 2, JSON.stringify(bg.map(x => x.p)));
    assert.equal(prio.find(x => x.post.includes('FEwhat_to_watch'))?.p, 'High', 'first page at normal priority');
    // Scrolling needs no further requests up to 100 cards.
    const n = log.api.length;
    await page.waitForFunction(() => { scrollBy(0, 1200); return document.querySelectorAll('#ytc-list .c').length >= 100; }, null, { timeout: 5000, polling: 50 });
    assert.ok(log.api.slice(n).filter(a => a.body?.continuation).length <= 1, 'scroll used preloaded items');
    await page.evaluate(() => scrollTo(0, 0));
  });

  await step('thumbnails: srcset w descriptors (vi_webp 320w … 720w), sizes, first row fetchpriority=high, lazy below, preconnect', async () => {
    const imgs = await page.$$eval('#ytc-list .c a.th img', els => els.map(e => ({ src: e.getAttribute('src'), srcset: e.getAttribute('srcset') || '', sizes: e.getAttribute('sizes') || '', loading: e.getAttribute('loading'), fp: e.getAttribute('fetchpriority'), dec: e.getAttribute('decoding') })));
    assert.ok(imgs.length >= 100);
    assert.ok(imgs.every(i => i.dec === 'async'));
    const f = imgs[0];
    assert.match(f.srcset, /^https:\/\/i\.ytimg\.com\/vi_webp\/[\w-]{11}\/mqdefault\.webp 320w, https:\/\/i\.ytimg\.com\/vi\/[^ ]+ 360w, https:\/\/i\.ytimg\.com\/vi\/[^ ]+ 720w$/, f.srcset);
    assert.ok(f.sizes.endsWith('460px'), f.sizes);
    assert.equal(f.loading, null, 'first screen not lazy');
    // 1400 px viewport → 4 grid columns (5 above 1700 px) → first row high priority.
    assert.deepEqual(imgs.slice(0, 6).map(i => i.fp), ['high', 'high', 'high', 'high', null, null]);
    assert.ok(imgs.slice(30).every(i => i.loading === 'lazy' && !i.fp));
    const pre = await page.$$eval('link[rel=preconnect]', ls => ls.map(l => l.href));
    // Image hosts from the shell; video servers are warmed separately once a player response is known.
    assert.deepEqual(pre.filter(h => !h.endsWith('.googlevideo.com/')), ['https://i.ytimg.com/', 'https://yt3.ggpht.com/']);
  });

  await step('Home reload: YouTube\'s answer on the first frame, from the ready response (fetched ahead once, used once, never the old Home first); expiry, sign-in change and watching discard it', async () => {
    await page.evaluate(() => scrollTo(0, 0));
    const wasRich = HOME.rich;
    HOME.rich = true; HOME.stamp = true;
    // Every document: the first frame that shows Home cards, and every write of #ytc-main.
    await page.addInitScript(() => {
      const w = window.__home = { first: null, frames: 0, writes: 0 };
      const tick = () => {
        const t = document.querySelector('#ytc-list .c[data-v] .t');
        if (t && w.first === null) { w.first = t.textContent; w.firstAt = performance.now(); w.upd = document.getElementById('ytc-main')?.dataset.upd || ''; }
        if (++w.frames < 2000) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      new MutationObserver(ms => { for (const m of ms) if (m.target.id === 'ytc-main') w.writes++; }).observe(document, { childList: true, subtree: true });
    });
    const homeReqs = () => log.api.filter(a => a.ep === 'browse' && a.body?.browseId === 'FEwhat_to_watch');
    const stamp = t => +((/Fresh #(\d+)/.exec(t || '') || [])[1] || 0);
    const ready = () => page.evaluate(() => { const r = JSON.parse(sessionStorage.getItem('ytc:ready') || 'null'); return r && { n: r.i.length, first: r.f, title: r.i[0].t, at: r.t, cont: !!r.c }; });
    const waitReady = () => page.waitForFunction(() => { const r = sessionStorage.getItem('ytc:ready'); return r && JSON.parse(r).i.length > 30; }, null, { timeout: 8000 });
    const home = () => page.evaluate(() => ({ ...window.__home, src: document.documentElement.dataset.ytcHomeSrc, title: document.querySelector('#ytc-list .c[data-v] .t')?.textContent }));
    try {
      // (A ready response fetched before the stamps were on would be the page: start without one.)
      await page.evaluate(() => sessionStorage.removeItem('ytc:ready'));
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForFunction(() => /Fresh #/.test(document.querySelector('#ytc-list .c .t')?.textContent || ''));
      // 1. After the paint, in idle time, the Home request once more (low priority) and its first continuation.
      await waitReady();
      const r1 = await ready();
      assert.ok(r1.first > 0 && r1.n > r1.first && r1.cont, `first page + its continuation ${JSON.stringify(r1)}`);
      const shown = stamp((await home()).title);
      assert.ok(stamp(r1.title) > shown, 'a new answer, not the one on screen');
      const lowReady = prio.filter(x => x.post.includes('FEwhat_to_watch')).at(-1);
      assert.equal(lowReady.p, 'Low', 'fetched at low priority');
      // 2. Reload: the ready response is the page on the first frame with cards, without a Home request
      //    before it; exactly one Home request afterwards (the next ready response), sent after the paint.
      HOME.delay = 700;
      const n0 = homeReqs().length, t0 = Date.now();
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => window.__home && window.__home.first !== null);
      let h = await home();
      assert.equal(stamp(h.first), stamp(r1.title), 'the first cards are the ready response');
      assert.equal(h.src, 'ready');
      assert.equal(await page.evaluate(() => document.documentElement.dataset.ytcRepaint), undefined, 'no snapshot repaint');
      assert.equal(await page.evaluate(() => sessionStorage.getItem('ytc:ready')), null, 'used up: taken out at once');
      const after = homeReqs().slice(n0);
      assert.ok(after.every(a => a.t - t0 > h.firstAt - 50), 'no Home request before the paint');
      // 3. Reloading again before the next one has landed: the skeleton and YouTube's answer (never the old Home first).
      await page.waitForTimeout(150);
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => window.__home && window.__home.first !== null, null, { timeout: 5000 });
      h = await home();
      assert.equal(h.src, 'network');
      assert.ok(stamp(h.first) > stamp(r1.title), `the first cards are a new answer (${h.first})`);
      HOME.delay = 0;
      // 4. Expired (older than 10 min): not used.
      await waitReady();
      await page.evaluate(() => { const r = JSON.parse(sessionStorage.getItem('ytc:ready')); r.t -= 11 * 60000; sessionStorage.setItem('ytc:ready', JSON.stringify(r)); });
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => window.__home && window.__home.first !== null);
      assert.equal((await home()).src, 'network', 'an expired ready response is not used');
      // 5. Signing in (another account key): not used.
      await waitReady();
      await ctx.addCookies([{ name: 'SAPISID', value: 'e2e-ready-account', domain: '.youtube.com', path: '/', secure: true }]);
      try {
        await page.reload({ waitUntil: 'commit' });
        await page.waitForFunction(() => window.__home && window.__home.first !== null);
        assert.equal((await home()).src, 'network', 'another account: not used');
      } finally { await ctx.clearCookies({ name: 'SAPISID' }); }
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => window.__home && window.__home.first !== null);
      // 6. Opening a video discards it (watching changes what YouTube recommends); nothing is fetched
      //    ahead on the watch page; Home afterwards asks YouTube.
      await waitReady();
      await page.click('#ytc-list a.t[href^="/watch"]');
      await page.waitForSelector('#ytc-info h1.wt');
      assert.equal(await page.evaluate(() => sessionStorage.getItem('ytc:ready')), null, 'watching discards it');
      const n1 = homeReqs().length;
      await page.waitForTimeout(2500);
      assert.equal(homeReqs().length, n1, 'nothing fetched ahead on a watch page');
      await page.mouse.move(700, 450);
      const tc = Date.now();
      await page.click('#ytc-logo');
      await page.waitForFunction(() => /Fresh #/.test(document.querySelector('#ytc-list .c .t')?.textContent || ''));
      assert.ok(homeReqs().length > n1 && homeReqs()[n1].t >= tc - 100, 'Home asked YouTube when it was opened');
      assert.notEqual(await page.evaluate(() => document.documentElement.dataset.ytcHomeSrc), 'ready');
      // 7. Feeds stay in this tab: the ready response is in sessionStorage only (checked with chrome.storage in the next steps).
      await waitReady();
      assert.deepEqual(await page.evaluate(() => Object.keys(localStorage).filter(k => k.startsWith('ytc:'))), []);
    } finally { HOME.delay = 0; HOME.stamp = false; HOME.rich = wasRich; }
  });

  await step('Home request hedged: a stalled first request gets an identical second one at the deadline; the first answer paints once and the other is aborted; past 4 s the old Home shows dimmed ("Updating…"), offline it waits for the connection', async () => {
    const wasRich = HOME.rich;
    HOME.rich = true; HOME.stamp = true;
    const homeReqs = () => log.api.filter(a => a.ep === 'browse' && a.body?.browseId === 'FEwhat_to_watch');
    const failed = [];
    const onFail = r => { if (r.url().includes('/youtubei/v1/browse')) failed.push(r.failure()?.errorText); };
    page.on('requestfailed', onFail);
    try {
      await page.evaluate(() => { localStorage.setItem('ytc-test-noready', '1'); sessionStorage.removeItem('ytc:ready'); sessionStorage.setItem('ytc:bt', '120,140,150,160,170,180'); });
      // 1. The first request hangs: the hedge goes out at this tab's p90 (clamped to ≥ 0.7 s) and paints.
      HOME.stall = 1; HOME.aborted = 0;
      const n0 = homeReqs().length;
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => window.__home && window.__home.first !== null, null, { timeout: 5000 });
      const h = await page.evaluate(() => ({ ...window.__home, hedge: document.documentElement.dataset.ytcHedge }));
      const reqs = homeReqs().slice(n0);
      assert.equal(reqs.length, 2, 'the request and its hedge');
      assert.equal(JSON.stringify(reqs[0].body), JSON.stringify(reqs[1].body), 'identical requests');
      const gap = reqs[1].t - reqs[0].t;
      assert.ok(gap >= 650 && gap < 1200, `hedge after ${gap} ms`);
      assert.equal(h.hedge, 'hedge', 'the hedge won');
      assert.ok(h.firstAt < 2500, `painted at ${h.firstAt.toFixed(0)} ms`);
      await page.waitForFunction(() => document.querySelector('#ytc-list .c[data-v]'));
      await page.waitForTimeout(300);
      assert.ok(failed.includes('net::ERR_ABORTED'), `the first request was aborted (${failed.join()})`);
      const writes = await page.evaluate(() => window.__home.writes);
      assert.ok(writes <= 2, `#ytc-main written ${writes} times (skeleton shell, then the page once)`);
      // 2. Both late (> 4 s): the old Home dimmed with "Updating…", replaced in place when YouTube answers.
      HOME.delay = 5200;
      await page.evaluate(() => sessionStorage.setItem('ytc:bt', '3000,3000,3000,3000,3000,3000'));
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => document.getElementById('ytc-main')?.dataset.upd === '1', null, { timeout: 6000 });
      const dim = await page.evaluate(() => ({ cards: document.querySelectorAll('#ytc-list .c[data-v]').length, op: getComputedStyle(document.querySelector('#ytc-main>.feed')).opacity, note: getComputedStyle(document.getElementById('ytc-main'), '::after').content, at: performance.now() }));
      assert.ok(dim.cards > 10 && +dim.op < 0.6 && /Updating/.test(dim.note) && dim.at >= 3900, JSON.stringify(dim));
      await page.waitForFunction(() => document.getElementById('ytc-main').dataset.upd === '' && document.documentElement.dataset.ytcFresh, null, { timeout: 4000 });
      HOME.delay = 0;
      // 3. Offline: the old Home dimmed with "No connection", probed every 3 s; back → it loads by itself.
      NET.down = true; NET.probes = 0;
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => document.getElementById('ytc-main')?.dataset.upd === '2', null, { timeout: 5000 });
      assert.match(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-main'), '::after').content), /No connection/);
      assert.equal(await page.evaluate(() => location.pathname), '/', 'stays in the client');
      NET.down = false;
      await page.waitForFunction(() => document.getElementById('ytc-main').dataset.upd === '' && document.documentElement.dataset.ytcFresh, null, { timeout: 7000 });
      assert.ok(NET.probes >= 1);
    } finally {
      page.off('requestfailed', onFail);
      HOME.stall = 0; HOME.delay = 0; HOME.stamp = false; HOME.rich = wasRich; NET.down = false;
      await page.evaluate(() => { localStorage.removeItem('ytc-test-noready'); sessionStorage.removeItem('ytc:bt'); }).catch(() => {});
    }
  });

  await step('back from another document restores Home at its scroll position without refetching', async () => {
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    // The fresh feed may show only its first chunk (page shorter than 1500 + viewport):
    // scroll on until more cards have been appended and the position sticks.
    await page.waitForFunction(() => { scrollTo(0, 1500); return scrollY === 1500; }, null, { timeout: 8000, polling: 50 });
    await page.waitForTimeout(100);
    await page.goto('https://www.youtube.com/results?search_query=back+test', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const n = apiCount('browse');
    await page.goBack({ waitUntil: 'commit' });
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length >= 30 && location.pathname === '/');
    await page.waitForTimeout(300);
    const y = await page.evaluate(() => scrollY);
    assert.ok(Math.abs(y - 1500) < 60, `scrollY ${y}`);
    assert.equal(apiCount('browse'), n, 'no Home request');
    await page.evaluate(() => scrollTo(0, 0));
  });

  await step('feeds and continuation tokens never reach chrome.storage or localStorage', async () => {
    const ls = await page.evaluate(() => Object.keys(localStorage).filter(k => k.startsWith('ytc:')));
    assert.deepEqual(ls, []);
    const dir = path.join(profile, 'Default', 'Local Extension Settings');
    const bytes = fs.readdirSync(dir).flatMap(x => fs.readdirSync(path.join(dir, x)).map(f => path.join(dir, x, f))).filter(f => fs.statSync(f).isFile()).map(f => fs.readFileSync(f).toString('latin1')).join('');
    assert.ok(bytes.includes('ytcConfigV1'), 'storage readable (config present)');
    assert.ok(!bytes.includes(HOME_ID0) && !bytes.includes('HOMECONT') && !bytes.includes('ytc:snap'), 'feed data in chrome.storage');
  });

  await step('logo: resting on it opens Home from memory (the ready response, else a request on enter; 250 ms dwell, mint pill fills); a pass-over or a shorter rest never navigates, a new rest starts over; click instant; keyboard unchanged; nothing while typing; Home on Home: top, then a new Home from YouTube', async () => {
    const saved = { ...HOME };
    HOME.rich = true; HOME.latency = 150; HOME.delay = 0;
    const box = await page.locator('#ytc-logo').boundingBox();
    const lc = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    const homeReqs = () => log.api.filter(a => a.ep === 'browse' && a.body?.browseId === 'FEwhat_to_watch');
    // Page-side watch: when the pointer entered the logo, whether the pill was filling mid-dwell, and the
    // first frame showing Home (cards from memory, or a skeleton waiting for the network).
    const watch = () => page.evaluate(() => {
      const w = window.__dw = { enter: null, mid: null, nav: null };
      document.getElementById('ytc-logo').addEventListener('pointerover', () => {
        if (w.enter !== null) return;
        w.enter = performance.now(); w.enterEpoch = Date.now();
        setTimeout(() => { const l = document.getElementById('ytc-logo'); w.mid = l && l.classList.contains('dw') && getComputedStyle(l, '::before').opacity === '1'; }, 120);
      });
      const tick = () => {
        if (location.pathname === '/') { w.nav = performance.now(); w.navEpoch = Date.now(); w.cards = document.querySelectorAll('#ytc-list .c[data-v]').length; w.sk = !!document.querySelector('#ytc-main .sk'); w.dw = document.getElementById('ytc-logo').classList.contains('dw'); }
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    const results = async q => {
      await page.mouse.move(700, 450);
      await page.goto('https://www.youtube.com/results?search_query=' + q, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.evaluate(() => { sessionStorage.removeItem('ytc:snap:/'); sessionStorage.removeItem('ytc:ready'); });
    };
    try {
      // 1. Rest on the logo from a results page (no Home snapshot): Home's first page is requested the moment
      // the pointer enters (not after the 70 ms hover delay) and the dwell ends in Home painted from memory.
      // (The background refresh, which would have fetched Home already, is off for this part.)
      await page.evaluate(() => { localStorage.setItem('ytc-test-nobg', '1'); localStorage.setItem('ytc-test-noready', '1'); });
      const dwellMs = [];
      for (let i = 0; i < 3; i++) {
        await results('dwell' + i);
        await watch();
        const n0 = homeReqs().length;
        await page.mouse.move(lc.x, lc.y);
        await page.waitForFunction(() => window.__dw.nav !== null, null, { timeout: 3000 });
        const w = await page.evaluate(() => window.__dw);
        const reqs = homeReqs().slice(n0);
        assert.equal(reqs.length, 1, 'one Home request, sent on enter (none at the end of the dwell)');
        assert.ok(reqs[0].t - w.enterEpoch < 50, `Home requested ${reqs[0].t - w.enterEpoch} ms after entering the logo`);
        assert.ok(w.cards >= 20 && !w.sk, `Home painted from memory on the dwell's frame (cards ${w.cards}, skeleton ${w.sk})`);
        assert.ok(w.nav - w.enter >= 240 && w.nav - w.enter < 450, `dwell ${(w.nav - w.enter).toFixed(0)} ms`);
        assert.equal(w.mid, true, 'the pill fills during the dwell');
        assert.equal(w.dw, false, 'the pill is gone when Home opens');
        dwellMs.push(w.nav - w.enter);
        // Still resting there: no second dwell (the pointer has to leave first).
        const h = await page.evaluate(() => history.length);
        await page.waitForTimeout(400);
        assert.equal(await page.evaluate(() => history.length), h, 'no second dwell while the pointer stays');
      }
      await page.evaluate(() => { localStorage.removeItem('ytc-test-nobg'); localStorage.removeItem('ytc-test-noready'); });
      console.log(`     logo rest → Home cards (pointer enters the logo; 150 ms InnerTube latency): median ${median(dwellMs).toFixed(0)} ms`);
      // 1b. With a ready Home response waiting, the dwell paints it: no request on enter or at the end
      // (only the next ready response afterwards, once Home is on screen).
      HOME.stamp = true;
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForFunction(() => { const r = sessionStorage.getItem('ytc:ready'); return r && JSON.parse(r).i.length > 30; }, null, { timeout: 8000 });
      const readyTitle = await page.evaluate(() => JSON.parse(sessionStorage.getItem('ytc:ready')).i[0].t);
      await page.mouse.move(700, 450);
      await page.goto('https://www.youtube.com/results?search_query=dwellready', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await watch();
      const n0 = homeReqs().length;
      await page.mouse.move(lc.x, lc.y);
      await page.waitForFunction(() => window.__dw.nav !== null, null, { timeout: 3000 });
      const wr = await page.evaluate(() => window.__dw);
      const sent = homeReqs().filter(a => a.t <= wr.navEpoch + 5);
      assert.equal(sent.length, n0, 'no Home request on enter or at the end of the dwell');
      assert.ok(wr.cards >= 20 && !wr.sk, 'painted on the dwell\'s frame');
      assert.equal(await page.evaluate(() => document.querySelector('#ytc-list .c[data-v] .t').textContent), readyTitle, 'the ready response');
      HOME.stamp = false;
      // 2. Passing over (sidebar → search, fast and slow) and resting shorter than the dwell never navigate.
      await results('pass');
      await page.mouse.move(4, lc.y);
      await page.mouse.move(lc.x + 400, lc.y, { steps: 8 });
      for (let x = box.x - 20; x < box.x + box.width + 30; x += 30) { await page.mouse.move(x, lc.y); await page.waitForTimeout(20); }
      await page.mouse.move(lc.x + 400, lc.y + 300);
      await page.waitForTimeout(450);
      assert.equal(await page.evaluate(() => location.pathname), '/results', 'a pass-over did not navigate');
      assert.equal(await page.evaluate(() => document.getElementById('ytc-logo').classList.contains('dw')), false, 'the pill retracted on leaving');
      // A new rest starts over: 150 ms, out, 150 ms again → still here; then the full dwell → Home.
      await page.mouse.move(lc.x, lc.y); await page.waitForTimeout(150);
      await page.mouse.move(lc.x, lc.y + 60); await page.waitForTimeout(40);
      await page.mouse.move(lc.x, lc.y); await page.waitForTimeout(150);
      assert.equal(await page.evaluate(() => location.pathname), '/results', 'leaving restarted the dwell');
      await page.waitForFunction(() => location.pathname === '/', null, { timeout: 1000 });
      // 3. A click is instant (the dwell started by the pointer's arrival is dropped).
      await results('click');
      const h0 = await page.evaluate(() => history.length);
      await page.mouse.move(lc.x, lc.y);
      const t = await page.evaluate(() => new Promise(res => { const t0 = performance.now(); document.getElementById('ytc-logo').click(); res({ path: location.pathname, cards: document.querySelectorAll('#ytc-list .c').length, ms: performance.now() - t0 }); }));
      assert.equal(t.path, '/', 'the click navigated in its own task');
      await page.waitForTimeout(400);
      assert.equal(await page.evaluate(() => history.length), h0 + 1, 'one history entry: the dwell did not navigate again');
      // 4. Keyboard unchanged: focus + Enter opens Home at once (no pill).
      await results('keys');
      await page.focus('#ytc-logo');
      await page.keyboard.press('Enter');
      assert.equal(await page.evaluate(() => location.pathname), '/');
      assert.equal(await page.evaluate(() => document.getElementById('ytc-logo').classList.contains('dw')), false);
      // 5. Nothing while typing in the search field (or with its dropdown open).
      await results('typing');
      await page.click('#ytc-q'); await page.keyboard.type('abc');
      await page.mouse.move(lc.x, lc.y); await page.waitForTimeout(450);
      assert.equal(await page.evaluate(() => location.pathname), '/results', 'no dwell while typing');
      assert.equal(await page.evaluate(() => document.getElementById('ytc-logo').classList.contains('dw')), false);
      if (process.env.YTC_SHOTS) {
        // The cue mid-dwell (125 of 250 ms), frozen: the pointer rests on the logo (its hover look) while typing
        // keeps the real dwell from running; the class is the one Rust sets.
        await page.evaluate(() => { document.getElementById('ytc-logo').classList.add('dw'); });
        await page.evaluate(() => { for (const a of document.getAnimations()) if (a.transitionProperty === 'clip-path' || a.transitionProperty === 'opacity') { a.pause(); a.currentTime = 125; } });
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'logo-dwell-mid.png'), clip: { x: box.x - 24, y: box.y - 16, width: box.width + 48, height: box.height + 32 } });
        await page.evaluate(() => { for (const a of document.getAnimations()) a.finish(); });
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'logo-dwell-full.png'), clip: { x: box.x - 24, y: box.y - 16, width: box.width + 48, height: box.height + 32 } });
        await page.evaluate(() => document.getElementById('ytc-logo').classList.remove('dw'));
      }
      await page.keyboard.press('Escape');
      await page.evaluate(() => document.activeElement.blur());
      await page.mouse.move(lc.x, lc.y + 300);
      await page.mouse.move(lc.x, lc.y);
      await page.waitForFunction(() => location.pathname === '/', null, { timeout: 1000 });
      // 5b. Watch pages scroll the top bar away: the logo coming back under a resting pointer is no rest on it.
      await results('scrollunder');
      const href = await page.getAttribute('#ytc-list a.t[href^="/watch"]', 'href');
      await page.goto('https://www.youtube.com' + href, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c');
      await page.evaluate(() => scrollTo(0, 400));
      await page.waitForTimeout(100);
      await page.mouse.move(lc.x, lc.y); await page.waitForTimeout(400);
      const away = await page.evaluate(([x, y]) => !document.elementFromPoint(x, y)?.closest('#ytc-logo'), [lc.x, lc.y]);
      await page.evaluate(() => { window.__over = 0; document.getElementById('ytc-logo').addEventListener('pointerover', () => window.__over++); scrollTo(0, 0); });
      await page.waitForTimeout(550);
      const back = await page.evaluate(([x, y]) => ({ on: !!document.elementFromPoint(x, y)?.closest('#ytc-logo'), over: window.__over, dw: document.getElementById('ytc-logo').classList.contains('dw') }), [lc.x, lc.y]);
      assert.ok(away && back.on, 'the top bar scrolled away from the pointer and back under it');
      assert.equal(back.dw, false, 'no pill for a logo that came under a still pointer');
      assert.equal(await page.evaluate(() => location.pathname), '/watch', 'the logo scrolling under a still pointer did not navigate');
      console.log(`     scroll-under: pointerover events on the logo after the scroll: ${back.over}`);
      await page.mouse.move(lc.x + 3, lc.y);
      await page.waitForFunction(() => location.pathname === '/', null, { timeout: 1000 });
      // 6. Home on Home (like YouTube's logo on its Home): back to the top at once, no history entry, and a new
      // Home from YouTube: the ready response when one is waiting (no request), else the request sent on enter.
      HOME.stamp = true;
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.mouse.move(700, 450);
      await page.waitForFunction(() => { const r = sessionStorage.getItem('ytc:ready'); return r && JSON.parse(r).i.length > 30; }, null, { timeout: 8000 });
      const next = await page.evaluate(() => JSON.parse(sessionStorage.getItem('ytc:ready')).i[0].t);
      await page.evaluate(() => scrollTo(0, 900));
      const before = { h: await page.evaluate(() => history.length), n: homeReqs().length };
      await page.mouse.move(lc.x, lc.y); await page.waitForTimeout(400);
      let s = await page.evaluate(() => ({ y: scrollY, h: history.length, path: location.pathname, t: document.querySelector('#ytc-list .c[data-v] .t').textContent }));
      assert.deepEqual(s, { y: 0, h: before.h, path: '/', t: next }, 'Home on Home: top, the ready response');
      assert.ok(homeReqs().slice(before.n).every(a => a.t > Date.now() - 350), 'no request at the dwell (only the next ready response after it)');
      // Without a ready response, the click asks YouTube and replaces the page in place.
      await page.evaluate(() => { localStorage.setItem('ytc-test-noready', '1'); sessionStorage.removeItem('ytc:ready'); scrollTo(0, 900); });
      const k0 = HOME.served, n1 = homeReqs().length;
      await page.evaluate(() => document.getElementById('ytc-logo').click());
      s = await page.evaluate(() => ({ y: scrollY, h: history.length }));
      assert.deepEqual(s, { y: 0, h: before.h }, 'click on Home: top at once, no history entry');
      await page.waitForFunction(k => { const m = /Fresh #(\d+)/.exec(document.querySelector('#ytc-list .c .t')?.textContent || ''); return m && +m[1] > k; }, k0, { timeout: 2000 });
      assert.equal(homeReqs().length, n1 + 1, 'one request');
    } finally {
      Object.assign(HOME, saved);
      await page.evaluate(() => { sessionStorage.removeItem('ytc:bgage'); localStorage.removeItem('ytc-test-nobg'); localStorage.removeItem('ytc-test-noready'); }).catch(() => {});
      await page.mouse.move(700, 450);
    }
  });

  await step('search box dropdown: history (recent on focus, keyboard, remove) + suggestions, escaped', async () => {
    await page.fill('#ytc-q', '');
    await page.click('#ytc-q');
    // 'xss' was submitted through the top bar earlier: recent searches show on focus.
    await page.waitForSelector('#ytc-sug li.hs');
    assert.ok((await page.locator('#ytc-sug li.hs span').allTextContents()).includes('xss'));
    assert.equal(await page.getAttribute('#ytc-q', 'aria-expanded'), 'true');
    await page.type('#ytc-q', 'rob', { delay: 30 });
    await page.waitForFunction(() => document.querySelectorAll('#ytc-sug li:not(.hs)').length === 2);
    const vals = await page.$$eval('#ytc-sug li:not(.hs) span', o => o.map(x => x.textContent));
    assert.deepEqual(vals, ['roblox', 'roblox <b>evil</b>" x="']);
    assert.equal(await page.locator('#ytc-sug b, #ytc-sug [x]').count(), 0);
    const u = new URL(log.suggest.at(-1));
    assert.equal(u.origin + u.pathname, 'https://www.youtube.com/complete/search');
    assert.equal(u.searchParams.get('q'), 'rob');
    // Submit, then it is the first history match for "rob".
    await page.fill('#ytc-q', 'roblox lua tables');
    await page.press('#ytc-q', 'Enter');
    await page.waitForFunction(() => location.search === '?search_query=roblox+lua+tables' || location.search === '?search_query=roblox%20lua%20tables');
    await page.waitForSelector('#ytc-list .c');
    await page.fill('#ytc-q', '');
    await page.click('#ytc-q');
    await page.type('#ytc-q', 'rob', { delay: 20 });
    await page.waitForSelector('#ytc-sug li.hs');
    assert.equal(await page.locator('#ytc-sug li.hs span').first().textContent(), 'roblox lua tables');
    // Arrow keys select rows (input shows the row), Delete removes a history row.
    await page.press('#ytc-q', 'ArrowDown');
    assert.equal(await page.inputValue('#ytc-q'), 'roblox lua tables');
    assert.equal(await page.getAttribute('#ytc-q', 'aria-activedescendant'), 'ytc-sg0');
    assert.equal(await page.locator('#ytc-sug li.on').count(), 1);
    await page.press('#ytc-q', 'Delete');
    await page.waitForFunction(() => ![...document.querySelectorAll('#ytc-sug li.hs span')].some(s => s.textContent === 'roblox lua tables'));
    assert.equal(await page.inputValue('#ytc-q'), 'rob', 'typed text back after removing');
    // Escape closes.
    await page.press('#ytc-q', 'Escape');
    assert.equal(await page.locator('#ytc-sug li').count(), 0);
    // Mouse: remove button keeps the box focused; a row click searches.
    await page.fill('#ytc-q', '');
    await page.click('#ytc-q');
    await page.waitForSelector('#ytc-sug li.hs');
    const before = await page.locator('#ytc-sug li.hs').count();
    await page.hover('#ytc-sug li.hs >> nth=0');
    await page.click('#ytc-sug li.hs >> nth=0 >> .sgx');
    await page.waitForFunction(n => document.querySelectorAll('#ytc-sug li.hs').length === n - 1, before);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'ytc-q');
    if (before > 1) {
      const q = await page.locator('#ytc-sug li.hs span').first().textContent();
      await page.click('#ytc-sug li.hs >> nth=0 >> a');
      await page.waitForFunction(q => document.title.startsWith(q), q);
    }
    await page.fill('#ytc-q', '');
    await page.keyboard.press('Escape');
  });

  await step('hover: animated preview on a thumbnail; channel links prefetched and reused on click', async () => {
    await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const th = page.locator('#ytc-list .c a.th[href^="/watch"]').first();
    await th.hover();
    await page.waitForSelector('#ytc-list a.th img.pv', { timeout: 3000 });
    assert.ok((await page.getAttribute('#ytc-list a.th img.pv', 'src')).startsWith('https://i.ytimg.com/an_webp/'));
    await page.mouse.move(2, 2);
    await page.waitForSelector('img.pv', { state: 'detached', timeout: 2000 });
    // (It may already be in the 2-minute prefetch cache from an earlier hover.)
    const ch = page.locator('#ytc-list .c:not(.plc):not(.chc) .m a[href^="/@"]').first();
    const href = await ch.getAttribute('href');
    await ch.hover();
    await page.waitForTimeout(300);
    const n = apiCount('browse');
    await ch.click();
    await page.waitForSelector('.ch h1');
    assert.equal(new URL(page.url()).pathname, href);
    assert.equal(apiCount('browse'), n, 'click rendered the hover-prefetched channel');
    await page.keyboard.press('Backquote');
    assert.match(await page.locator('#ytc-debug').textContent(), /prefetched/);
    await page.keyboard.press('Backquote');
    HOME.rich = false;
  });

  await step('/shorts/ID opens as a normal watch page', async () => {
    await page.goto('https://www.youtube.com/shorts/DQcrB2wLu04', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-video');
    assert.equal(new URL(page.url()).pathname + new URL(page.url()).search, '/watch?v=DQcrB2wLu04');
    await page.waitForFunction(() => document.documentElement.dataset.ytcFirstFrame);
  });

  const vstate = () => page.evaluate(() => { const v = document.getElementById('ytc-video'); if (!v) return null;
    const q = v.getVideoPlaybackQuality(); return { t: v.currentTime, paused: v.paused, ended: v.ended, rs: v.readyState, h: v.videoHeight, rate: v.playbackRate, dropped: q.droppedVideoFrames, frames: q.totalVideoFrames }; });
  const waitT = (pred, arg, timeout = 10000) => page.waitForFunction(pred, arg, { timeout, polling: 100 });
  // The watch-open morph has landed (layout assertions read the painted box).
  const morphDone = () => page.waitForFunction(() => !document.getElementById('ytc-player')?.getAnimations().some(a => a.id === 'flip'), null, { timeout: 3000 });
  const shot = async name => { if (process.env.YTC_SHOTS) await page.screenshot({ path: path.join(process.env.YTC_SHOTS, name + '.png'), animations: 'disabled' }); };
  const keys = async (k, n = 1) => { for (let i = 0; i < n; i++) await page.keyboard.press(k); };
  // 90 % through the video by a seek-bar click (0–9 are blocked keys now), then the pointer leaves.
  const nearEnd = async () => { const b = await page.locator('.pseek').boundingBox(); await page.mouse.click(b.x + b.width * 0.9, b.y + b.height / 2); await page.mouse.move(5, 5); };

  await step('timestamp: t= starts our player at that time; the description lists no timestamp links', async () => {
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0&t=30', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-info h1.wt');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame);
    await waitT(() => document.getElementById('ytc-video').currentTime >= 30);
    await page.waitForSelector('#ytc-desc:not(.sk)');
    assert.equal(await page.locator('#ytc-desc a[data-act="seek"]').count(), 0, 'timestamps are not description resources');
  });

  const ff = { spa: [] };
  await step('own player: first frame from VISIONOS streams, WEB metadata call, no YouTube player JS', async () => {
    log.gv.length = 0; log.stats.length = 0;
    const assets = log.assets.length, mark0 = log.api.length;
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-video');
    assert.equal(await page.getAttribute('#ytc-video', 'poster'), 'https://i.ytimg.com/vi_webp/aqz-KE-bpKQ/mqdefault.webp', 'thumbnail poster at once');
    // The caption cue's quick fade-in (a cue at 0 s can show before the first frame) is not a loading indicator.
    assert.equal(await page.evaluate(() => [...document.querySelectorAll('#ytc-player *:not(#ytc-cc)')].filter(e => getComputedStyle(e).animationName !== 'none').length), 0, 'no loading spinner/animation in the player');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    ff.direct = await page.evaluate(() => +document.documentElement.dataset.ytcFirstFrame);
    const players = log.api.slice(mark0).filter(a => a.ep === 'player' && a.body.videoId === 'aqz-KE-bpKQ');
    const vis = players.find(a => a.body.context.client.clientName === 'VISIONOS');
    const web = players.find(a => a.body.context.client.clientName === 'WEB');
    assert.ok(vis && web, 'both player calls');
    assert.equal(players.length, 2, 'happy path: one VISIONOS + one WEB player call');
    assert.equal(log.api.filter(a => a.ep === 'visitor_id').length, 0, 'happy path: no visitor_id');
    assert.ok(!vis.headers.authorization && !vis.headers.cookie, 'happy path: VISIONOS anonymous');
    assert.equal(vis.headers['x-youtube-client-name'], '101');
    assert.ok(vis.headers['x-goog-visitor-id']?.startsWith('Cg'), 'visitor data');
    assert.equal(vis.body.context.client.visitorData, vis.headers['x-goog-visitor-id']);
    assert.equal(web.headers['x-youtube-client-name'], '1');
    assert.ok(web.body.playbackContext.contentPlaybackContext.signatureTimestamp >= 20753);
    assert.equal(web.body.cpn.length, 16);
    // First segment rides along with the index (one request per track before the first frame).
    const first = log.gv.filter(g => g.a === 0);
    assert.ok(first.some(g => g.itag === '249' && g.b > +media.formats.find(f => f.itag === 249).indexRange.end), 'audio head + first segment');
    const st = await vstate();
    assert.ok(st.h > 0 && st.rs >= 2, JSON.stringify(st));
    assert.equal(log.assets.length, assets, 'no /s/ assets');
    assert.equal(await page.locator('iframe').count(), 0, 'no iframe');
  });

  await step('own player: plays ≥ 10 s; speed with < > (2×)', async () => {
    await waitT(() => document.getElementById('ytc-video').currentTime >= 3);
    await keys('>', 4);
    let st = await vstate();
    assert.equal(st.rate, 2);
    assert.equal(await page.locator('#ytc-speed').textContent(), '2×');
    await waitT(() => document.getElementById('ytc-video').currentTime >= 10, null, 8000);
    st = await vstate();
    assert.ok(!st.paused && st.rs >= 3, JSON.stringify(st));
  });

  await step('own player: seek by clicking the seek bar and with j', async () => {
    const box = await page.locator('.pseek').boundingBox();
    await page.mouse.click(box.x + box.width * 0.75, box.y + box.height / 2);
    await waitT(() => { const v = document.getElementById('ytc-video'); return v.currentTime > 30.5 && v.readyState >= 3; });
    await keys('j');
    await waitT(() => { const v = document.getElementById('ytc-video'); return v.currentTime > 19 && v.currentTime < 25; });
    // 0–9 are blocked: no jump, the key is taken (default prevented), nothing else happens.
    const t0 = await page.evaluate(() => document.getElementById('ytc-video').currentTime);
    const prevented = await page.evaluate(() => ['0', '5', '9'].map(k => { const e = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }); document.body.dispatchEvent(e); return e.defaultPrevented; }));
    await keys('0'); await keys('9'); await keys('5');
    await page.waitForTimeout(300);
    const t1 = await page.evaluate(() => document.getElementById('ytc-video').currentTime);
    assert.deepEqual(prevented, [true, true, true], 'digits taken on the watch page');
    assert.ok(t1 >= t0 && t1 - t0 < 2, `no jump on 0/9/5: ${t0} → ${t1}`);
    await keys('<', 4);
    assert.equal((await vstate()).rate, 1);
  });

  await step('own player: quality switch from the menu (360p → 144p) during playback', async () => {
    assert.equal((await vstate()).h, 360, 'Auto picked the best rung that fits');
    assert.equal(await page.textContent('#ytc-qlbl'), '360', 'the Quality button shows the playing rung');
    if (process.env.YTC_SHOTS) { await page.mouse.move(700, 400); await page.waitForTimeout(100); const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'player-toolbar.png'), clip: { x: b.x, y: b.y + b.height - 70, width: b.width, height: 70 } }); }
    const w0 = await page.evaluate(() => [document.getElementById('ytc-qual').getBoundingClientRect().width, document.getElementById('ytc-speed').getBoundingClientRect().x]);
    await page.click('#ytc-qual');
    await page.waitForSelector('#ytc-player.menu #ytc-menu button');
    await page.locator('#ytc-menu button', { hasText: '144p' }).click();
    await waitT(() => document.getElementById('ytc-video').videoHeight === 144, null, 9000);
    const st = await vstate();
    assert.ok(!st.paused && st.rs >= 3, JSON.stringify(st));
    assert.ok(log.gv.some(g => g.itag === '278' && g.a > 0), '144p segments fetched');
    assert.equal(await page.textContent('#ytc-qlbl'), '144');
    const w1 = await page.evaluate(() => [document.getElementById('ytc-qual').getBoundingClientRect().width, document.getElementById('ytc-speed').getBoundingClientRect().x]);
    assert.deepEqual(w1, w0, 'label changes never move or resize the buttons');
    assert.equal(await page.evaluate(() => document.getElementById('ytc-player').scrollLeft + document.getElementById('ytc-player').scrollTop), 0, 'the player box never scrolls');
  });

  await step('own player: captions (json3 overlay) and history pings', async () => {
    assert.ok(await page.locator('#ytc-player.cc').count(), 'captions on by default');
    await waitT(() => /^Caption line \d+ <b>&amp;<\/b>$/.test(document.getElementById('ytc-cc').textContent), null, 5000);
    assert.equal(await page.locator('#ytc-cc b').count(), 0, 'caption text is not HTML');
    assert.ok(log.timedtext.some(u => u.includes('fmt=json3') && u.includes('lang=en')));
    await keys('k');
    await waitT(() => document.getElementById('ytc-video').paused);
    const paused = () => log.stats.filter(u => u.includes('/watchtime') && u.includes('state=paused')).pop();
    for (let i = 0; i < 20 && !paused(); i++) await page.waitForTimeout(100);
    const pb = log.stats.find(u => u.includes('/api/stats/playback'));
    const wt = paused();
    assert.ok(pb && wt, log.stats.join('\n'));
    assert.ok(pb.startsWith('https://www.youtube.com/api/stats/playback?cl=1&docid=aqz-KE-bpKQ'), 's.youtube.com → www');
    const p = new URL(pb).searchParams, w = new URL(wt).searchParams;
    assert.equal(p.get('ver'), '2');
    assert.equal(p.get('cpn').length, 16);
    assert.equal(p.get('c'), 'WEB');
    assert.equal(w.get('cpn'), p.get('cpn'));
    assert.equal(w.get('state'), 'paused');
    assert.ok(w.get('st') && w.get('et') && w.get('st').split(',').length === w.get('et').split(',').length, wt);
    assert.ok(Math.max(...w.get('et').split(',').map(Number)) > 20, 'played time reported');
  });

  await step('own player: end of stream (ended fires at the end; the app gets Out::Ended once)', async () => {
    await page.evaluate(() => delete document.documentElement.dataset.ytcEnded);
    // (0–9 are blocked now: jump near the end with the seek bar.)
    const sb = await page.locator('.pseek').boundingBox();
    await page.mouse.click(sb.x + sb.width * 0.9, sb.y + sb.height / 2);
    await page.mouse.move(5, 5);
    await keys('k');
    await waitT(() => document.getElementById('ytc-video').ended, null, 8000);
    const st = await vstate();
    assert.ok(st.t > SECONDS - 1, JSON.stringify(st));
    await waitT(() => document.documentElement.dataset.ytcEnded, null, 2000);
  });

  await step('own player: captions on + quality remembered across a reload (chrome.storage)', async () => {
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ&t=4', { waitUntil: 'commit' });
    await waitT(() => document.getElementById('ytc-cc')?.textContent.startsWith('Caption line'), null, 8000);
    assert.ok(await page.locator('#ytc-player.cc').count());
    await waitT(() => document.getElementById('ytc-video').videoHeight === 144);
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button', { hasText: 'Auto' }).click();
    await keys('c');
    assert.equal(await page.locator('#ytc-player.cc').count(), 0);
  });

  await step('captions: Japanese source → English translation (tlang=en) by default; a deliberate choice sticks for that video only', async () => {
    const lastCc = () => log.timedtext.filter(u => u.includes('v=ccjapanese1')).pop() || '';
    await page.goto('https://www.youtube.com/watch?v=ccjapanese1', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    const wasOn = await page.evaluate(() => document.getElementById('ytc-player').classList.contains('cc'));
    if (!wasOn) await keys('c');
    await page.waitForFunction(() => document.getElementById('ytc-player').classList.contains('cc'));
    for (let i = 0; i < 40 && !lastCc(); i++) await page.waitForTimeout(100);
    assert.match(lastCc(), /lang=ja&tlang=en&fmt=json3$/, 'English translation of the human Japanese track');
    await page.click('#ytc-ccb');
    const items = page.locator('#ytc-menu .mls button');
    assert.deepEqual(await items.allTextContents(), ['Japanese (auto-generated)', 'Japanese', 'English (translated)']);
    assert.equal(await page.locator('#ytc-menu .mls button.on').textContent(), 'English (translated)');
    await items.nth(1).click();
    for (let i = 0; i < 40 && !/lang=ja&fmt/.test(lastCc()); i++) await page.waitForTimeout(100);
    assert.match(lastCc(), /lang=ja&fmt=json3$/);
    await page.keyboard.press('Escape');
    // Another video keeps the fixed preference; coming back (memory) keeps the choice.
    await page.locator('#ytc-list a[href^="/watch"]').first().click();
    await waitT(() => !location.search.includes('ccjapanese1'), null, 5000);
    const n = log.timedtext.length;
    await page.evaluate(() => history.back());
    await waitT(() => location.search.includes('ccjapanese1'), null, 5000);
    for (let i = 0; i < 40 && !log.timedtext.slice(n).some(u => u.includes('v=ccjapanese1')); i++) await page.waitForTimeout(100);
    assert.match(lastCc(), /lang=ja&fmt=json3$/, 'remembered for this video');
    if (!wasOn) {
      await keys('c');
      await page.waitForFunction(() => !document.getElementById('ytc-player').classList.contains('cc'));
    }
  });

  const rate = async () => (await vstate()).rate;
  await step('player menus: hover opens; Speed/Quality are vertical lists centred on their shortcut (1× / Auto at the bottom); passing over rows selects nothing, resting selects (dwell), a click is instant; departure closes; keyboard + Escape', async () => {
    await page.mouse.move(5, 5);
    await keys('<', 10); // back to the lowest step, then
    await keys('>', 3);  // 1×
    assert.equal(await rate(), 1);
    await page.hover('#ytc-speed');
    await page.waitForSelector('#ytc-player.menu #ytc-menu .msp', { timeout: 1000 });
    assert.equal(await page.locator('#ytc-menu input[type=range]').count(), 0, 'no horizontal slider');
    assert.deepEqual(await page.locator('#ytc-mpre button').allTextContents(), ['2.5×', '2×', '1.75×', '1.5×', '1.25×', '1×'], 'top → bottom, 1× next to the shortcut');
    // Geometry: the panel is centred over its shortcut and above it; the rows are one aligned column, top to bottom.
    const geo = id => page.evaluate(([sc, list]) => {
      const r = el => el.getBoundingClientRect(), m = r(document.getElementById('ytc-menu')), s = r(document.getElementById(sc));
      const rows = [...document.querySelectorAll('#' + list + ' button')].map(r);
      return { dc: (m.left + m.width / 2) - (s.left + s.width / 2), above: s.top - m.bottom, ys: rows.map(b => b.top), ls: rows.map(b => b.left), ws: rows.map(b => b.width) };
    }, id);
    let g = await geo(['ytc-speed', 'ytc-mpre']);
    assert.ok(Math.abs(g.dc) <= 1 && g.above > 0, 'Speed panel centred above its shortcut ' + JSON.stringify(g));
    assert.ok(g.ys.every((y, i) => !i || y > g.ys[i - 1]) && g.ls.every(x => Math.abs(x - g.ls[0]) < 0.5) && g.ws.every(w => Math.abs(w - g.ws[0]) < 0.5), 'rows aligned in one column ' + JSON.stringify(g));
    // Passing over rows on the way does not select: sweep 1× → 2× quickly (the rows in between are crossed).
    const row = l => page.locator('#ytc-mpre button', { hasText: new RegExp('^' + l.replace('.', '\\.') + '$') });
    const mid = async l => { const b = await row(l).boundingBox(); return [b.x + b.width / 2, b.y + b.height / 2]; };
    let [x, y] = await mid('1×');
    await page.mouse.move(x, y);
    [x, y] = await mid('2×');
    await page.mouse.move(x, y, { steps: 8 });
    await page.waitForTimeout(110);
    assert.equal(await rate(), 1, 'rows passed over select nothing (and the rested one not yet)');
    const fill = await page.evaluate(() => { const b = document.querySelector('#ytc-mpre .dw'); return b && { label: b.textContent, clip: getComputedStyle(b, '::before').clipPath, op: getComputedStyle(b, '::before').opacity }; });
    assert.ok(fill && fill.label === '2×' && fill.op !== '0', 'the rested row fills while it waits ' + JSON.stringify(fill));
    if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-menu').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'player-menu-speed-dwell.png'), clip: { x: b.x - 40, y: b.y - 20, width: b.width + 80, height: b.height + 100 } }); }
    await page.waitForTimeout(200);
    assert.equal(await rate(), 2, 'resting ~220 ms selects it');
    assert.equal(await page.locator('#ytc-mpre button.on').textContent(), '2×');
    assert.equal(await page.locator('#ytc-mpre .dw').count(), 0, 'the fill is gone once chosen');
    // A click is instant (no dwell wait), and wins over the dwell it started.
    await row('1.5×').click();
    assert.equal(await rate(), 1.5, 'click: at once');
    assert.equal(await page.locator('#ytc-mpre button.on').textContent(), '1.5×');
    await page.click('#ytc-menu .mrt button[aria-label=Faster]');
    assert.equal(await rate(), 1.55);
    assert.equal(await page.locator('#ytc-rate').textContent(), '1.55×', 'the readout carries the exact rate');
    assert.equal(await page.locator('#ytc-mpre button.on').count(), 0, 'an off-list rate selects no row');
    await page.mouse.move(5, 5);
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    await keys('>', 8);
    assert.equal(await rate(), 2.5, '> stops at 2.5');
    assert.equal(await page.locator('#ytc-rateb').textContent(), '2.5×');
    await keys('<', 6);
    assert.equal(await rate(), 1);
    assert.equal(await page.locator('#ytc-rateb').textContent(), '1×', 'the Speed button always shows the rate');
    // Quality: best at the top, Auto at the bottom next to the shortcut; centred on its shortcut.
    await page.hover('#ytc-qual');
    await page.waitForSelector('#ytc-menu .mq', { timeout: 1000 });
    assert.deepEqual(await page.locator('#ytc-mq button').allTextContents(), ['360p', '240p', '144p', 'Auto']);
    g = await geo(['ytc-qual', 'ytc-mq']);
    assert.ok(Math.abs(g.dc) <= 1 && g.above > 0, 'Quality panel centred above its shortcut ' + JSON.stringify(g));
    assert.ok(g.ys.every((y, i) => !i || y > g.ys[i - 1]) && g.ls.every(x => Math.abs(x - g.ls[0]) < 0.5), 'rows aligned ' + JSON.stringify(g));
    // Moving to another shortcut switches menus by hover.
    await page.hover('#ytc-ccb');
    await page.waitForSelector('#ytc-menu .mcc', { timeout: 1000 });
    assert.equal(await page.locator('#ytc-menu .mtab').count(), 0, 'one audio track: no tabs');
    assert.equal(await page.getAttribute('#ytc-menu .msw', 'aria-checked'), 'false');
    await page.click('#ytc-menu .msw');
    await page.waitForSelector('#ytc-player.cc');
    assert.equal(await page.getAttribute('#ytc-menu .msw', 'aria-checked'), 'true');
    assert.ok(await page.locator('#ytc-player.menu').count(), 'stays open after a choice');
    await page.mouse.move(5, 5);
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    // Keyboard: Enter opens with focus inside, arrows move, Escape returns to the shortcut.
    await page.focus('#ytc-qual');
    await page.keyboard.press('Enter');
    await page.waitForSelector('#ytc-menu .mq');
    // Keyboard opens have no motion: fully shown on the frame it opens (motion law).
    assert.deepEqual(await page.evaluate(() => { const cs = getComputedStyle(document.getElementById('ytc-menu')); return [cs.opacity, cs.transform]; }), ['1', 'none'], 'keyboard open is instant');
    assert.ok(await page.evaluate(() => document.getElementById('ytc-menu').contains(document.activeElement)), 'focus in the menu');
    assert.equal(await page.evaluate(() => document.activeElement.textContent), 'Auto', 'the selected choice is focused');
    await page.keyboard.press('ArrowUp');
    assert.equal(await page.evaluate(() => document.activeElement.textContent), '144p', 'ArrowUp goes up the list');
    await page.mouse.move(5, 5);
    await page.waitForTimeout(500);
    assert.ok(await page.locator('#ytc-player.menu').count(), 'keyboard menu ignores pointer departure');
    await page.keyboard.press('Escape');
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached' });
    assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-menu')).visibility), 'hidden', 'Escape closes at once');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'ytc-qual');
  });

  await step('captions: drag moves them, the size range scales them (0.5–2×), both remembered as one object', async () => {
    await waitT(() => document.getElementById('ytc-cc').textContent.startsWith('Caption line'), null, 5000);
    const box = await page.locator('#ytc-player').boundingBox();
    const cc = await page.locator('#ytc-cc').boundingBox();
    const paused = (await vstate()).paused;
    await page.mouse.move(cc.x + 4, cc.y + 4);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.4 + 4, box.y + box.height * 0.5 + 4, { steps: 5 });
    await page.mouse.up();
    const v = () => page.evaluate(() => { const s = getComputedStyle(document.getElementById('ytc-player')); return { x: +s.getPropertyValue('--cx'), y: +s.getPropertyValue('--cy'), s: +s.getPropertyValue('--cs') }; });
    let c = await v();
    assert.ok(Math.abs(c.x - 0.4) < 0.03 && Math.abs(c.y - 0.5) < 0.03, JSON.stringify(c));
    assert.equal((await vstate()).paused, paused, 'dragging captions does not toggle playback');
    const cc2 = await page.locator('#ytc-cc').boundingBox();
    assert.ok(Math.abs(cc2.x - (box.x + box.width * 0.4)) < 12, 'caption moved');
    await page.evaluate(() => { const r = document.getElementById('ytc-ccs'); r.value = '9'; r.dispatchEvent(new Event('input', { bubbles: true })); });
    assert.equal((await v()).s, 2, 'size capped at 2×');
    await page.evaluate(() => { const r = document.getElementById('ytc-ccs'); r.value = '1.5'; r.dispatchEvent(new Event('input', { bubbles: true })); });
    assert.equal((await v()).s, 1.5);
    // The mouse wheel over the caption resizes it (up = larger, 5 % a notch) and never scrolls the page.
    const ccb = await page.locator('#ytc-cc').boundingBox();
    await page.mouse.move(ccb.x + ccb.width / 2, ccb.y + ccb.height / 2);
    const y0 = await page.evaluate(() => scrollY);
    await page.mouse.wheel(0, -100); await page.mouse.wheel(0, -100);
    await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.600');
    await page.mouse.wheel(0, 100);
    await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.550');
    assert.equal(await page.evaluate(() => scrollY), y0, 'the wheel did not scroll the page');
    assert.equal(await page.locator('#ytc-ccv').textContent(), '155%');
    for (let i = 0; i < 12; i++) await page.mouse.wheel(0, -100);
    await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '2.000');
    // Arrow keys while the size control is focused: ±5 %, no seeking or volume change.
    const before = await vstate();
    await page.focus('#ytc-ccs');
    await page.keyboard.press('ArrowDown');
    await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.950');
    for (let i = 0; i < 10; i++) await page.keyboard.press('ArrowDown');
    await page.keyboard.press('ArrowUp');
    await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.500');
    const after = await vstate();
    assert.ok(Math.abs(after.t - before.t) < 4, `no seek on the size control's arrows ${before.t} → ${after.t}`);
    await page.evaluate(() => document.activeElement.blur());
    await page.waitForTimeout(450); // batched preference write
    await page.reload({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-video');
    await waitT(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.500', null, 5000);
    c = await v();
    assert.ok(Math.abs(c.x - 0.4) < 0.03 && Math.abs(c.y - 0.5) < 0.03, 'position remembered ' + JSON.stringify(c));
  });

  await step('player zero clicks: the wheel over the Speed shortcut steps 1→1.25→1.5 with a toast and never scrolls the page; ends clamp with a nudge; the Speed readout steps 0.05; the wheel in the Quality menu moves the selection every step but switches the stream once it rests; dwell in Quality; keys unchanged', async () => {
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await waitT(() => { const v = document.getElementById('ytc-video'); return !v.paused && v.currentTime > 0.5 && v.videoHeight === 360; }, null, 8000);
    await page.evaluate(() => scrollTo(0, 0));
    assert.equal(await rate(), 1);
    const center = async sel => { const b = await page.locator(sel).boundingBox(); return [b.x + b.width / 2, b.y + b.height / 2]; };
    const rateIs = r => waitT(want => document.getElementById('ytc-video').playbackRate === want, r, 1000);
    const toast = () => page.textContent('#ytc-toast');
    // 1. The menu closed (no hover): a wheel on the Speed shortcut is taken (no page scroll) and steps up once.
    const synth = await page.evaluate(() => { const e = new WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }); document.getElementById('ytc-speed').dispatchEvent(e); return { prevented: e.defaultPrevented, open: document.getElementById('ytc-player').classList.contains('menu') }; });
    assert.deepEqual(synth, { prevented: true, open: false }, 'taken on the closed shortcut');
    assert.equal(await rate(), 1.25, 'one step up');
    assert.equal(await page.textContent('#ytc-rateb'), '1.25×');
    assert.equal(await toast(), '1.25×');
    await keys('<');
    assert.equal(await rate(), 1, 'keys unchanged and instant');
    // 2. A real wheel over the shortcut: 1 → 1.25 → 1.5, a toast each step, the page stays put.
    const y0 = await page.evaluate(() => scrollY);
    const [sx, sy] = await center('#ytc-speed');
    await page.mouse.move(sx, sy);
    const notch = async (dy, wait = 160) => { await page.mouse.wheel(0, dy); await page.waitForTimeout(wait); };
    await notch(-100);
    await rateIs(1.25);
    assert.equal(await toast(), '1.25×');
    await notch(-100);
    await rateIs(1.5);
    assert.equal(await toast(), '1.5×');
    assert.equal(await page.evaluate(() => scrollY), y0, 'the page never scrolls under the shortcut');
    assert.equal(await page.locator('#ytc-mpre button.on').textContent(), '1.5×', 'the open list follows');
    // The top end: 2.5× holds, the list nudges once.
    for (let i = 0; i < 3; i++) await notch(-100);
    await rateIs(2.5);
    await page.mouse.wheel(0, -100);
    const band = await page.evaluate(() => new Promise(res => requestAnimationFrame(() => res(document.getElementById('ytc-mpre').getAnimations().length))));
    assert.ok(band > 0, 'a nudge at the end');
    await page.waitForTimeout(160);
    assert.equal(await rate(), 2.5, 'clamped at the top');
    for (let i = 0; i < 6; i++) await notch(100);
    await rateIs(1);
    assert.equal(await page.locator('#ytc-mpre button.on').textContent(), '1×', 'clamped at the bottom (1×)');
    // The readout steps 0.05 (slower rates stay reachable with zero clicks).
    const [rx, ry] = await center('#ytc-rate');
    await page.mouse.move(rx, ry);
    await notch(100);
    await rateIs(0.95);
    assert.equal(await page.textContent('#ytc-rate'), '0.95×');
    await notch(-100);
    await rateIs(1);
    assert.equal(await page.evaluate(() => scrollY), y0, 'still no page scroll');
    // 3. Quality, menu open: the selection follows every step at once, the stream switches once, after the rest.
    await page.mouse.move(5, 5);
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    const [qx, qy] = await center('#ytc-qual');
    await page.mouse.move(qx, qy);
    await page.waitForSelector('#ytc-player.menu #ytc-menu .mq', { timeout: 1000 });
    const [ax, ay] = await center('#ytc-mq button.on');
    await page.mouse.move(ax, ay); // rests on Auto (already chosen: no dwell)
    const gv0 = log.gv.length, onRow = () => page.textContent('#ytc-mq button.on');
    const slid = () => page.evaluate(() => new Promise(res => requestAnimationFrame(() => requestAnimationFrame(() => { const m = document.querySelector('#ytc-mq > i.pmk'); res(m ? m.getAnimations().length : -1); }))));
    await page.mouse.wheel(0, -100);
    await waitT(() => document.querySelector('#ytc-mq button.on')?.textContent === '144p', null, 1000);
    assert.ok(await slid() > 0, 'the mark slides to 144p');
    assert.equal(await toast(), '144p');
    await page.waitForTimeout(60);
    await page.mouse.wheel(0, -100);
    const tw = Date.now();
    await waitT(() => document.querySelector('#ytc-mq button.on')?.textContent === '240p', null, 1000);
    assert.equal(await toast(), '240p');
    assert.deepEqual(log.gv.slice(gv0).filter(g => g.itag === '278' || g.itag === '242'), [], 'nothing switched while the wheel moves');
    await waitT(() => document.getElementById('ytc-video').videoHeight === 240, null, 9000);
    const got = log.gv.slice(gv0);
    assert.ok(!got.some(g => g.itag === '278'), 'the passed 144p was never fetched');
    const first = got.find(g => g.itag === '242');
    assert.ok(first && first.t - tw >= 150, 'switched once the wheel rested ' + (first && first.t - tw) + ' ms');
    assert.equal(await page.textContent('#ytc-qlbl'), '240');
    // 4. Dwell in Quality: resting on 360p selects it; then resting on Auto returns to Auto.
    const [hx, hy] = await center('#ytc-mq button >> nth=0');
    await page.mouse.move(hx, hy, { steps: 4 });
    await page.waitForTimeout(80);
    if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-menu').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'player-menu-quality-dwell.png'), clip: { x: b.x - 40, y: b.y - 20, width: b.width + 80, height: b.height + 100 } }); }
    assert.equal(await onRow(), '240p', 'not yet');
    await page.waitForTimeout(250);
    assert.equal(await onRow(), '360p', 'dwell chose 360p');
    await waitT(() => document.getElementById('ytc-video').videoHeight === 360, null, 9000);
    const [ax2, ay2] = await center('#ytc-mq button >> nth=-1');
    await page.mouse.move(ax2, ay2, { steps: 4 });
    await page.waitForTimeout(320);
    assert.equal(await onRow(), 'Auto', 'dwell chose Auto');
    // 5. The wheel elsewhere on the player still scrolls the page (only the shortcuts and menus take it).
    await page.mouse.move(5, 5);
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    const pl = await page.locator('#ytc-player').boundingBox();
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    await page.mouse.wheel(0, 200);
    await waitT(y => scrollY > y, y0, 2000);
    await page.evaluate(() => scrollTo(0, 0));
  });

  await step('player motion: menu clickable on its opening frame, Speed→Quality→CC sweep never stuck or doubled, idle hide eased + instant wake, toast on the key frame, 1:1 scrub, no long tasks', async () => {
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await waitT(() => { const v = document.getElementById('ytc-video'); return !v.paused && v.currentTime > 0.5; }, null, 8000);
    const center = async sel => { const b = await page.locator(sel).boundingBox(); return [b.x + b.width / 2, b.y + b.height / 2]; };
    const pl = await page.locator('#ytc-player').boundingBox();
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    // 1. The class flip that opens the menu is observed before the next paint: the panel is visible, takes pointers and
    //    has its content on that same frame (only its opacity/scale is still easing in).
    await page.evaluate(() => {
      const box = document.getElementById('ytc-player'), m = document.getElementById('ytc-menu');
      const mo = new MutationObserver(() => {
        if (!box.classList.contains('menu')) return;
        mo.disconnect();
        const b = m.querySelector('button'), r = b && b.getBoundingClientRect(), cs = getComputedStyle(m);
        const hit = r && document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
        window.__open = { content: !!m.querySelector('.msp'), vis: cs.visibility, pe: cs.pointerEvents, hit: !!hit && (hit === b || b.contains(hit)), op: +cs.opacity, origin: cs.transformOrigin, mo: m.dataset.m };
      });
      mo.observe(box, { attributes: true, attributeFilter: ['class'] });
    });
    const [sx, sy] = await center('#ytc-speed');
    await page.mouse.move(sx, sy, { steps: 3 });
    await waitT(() => window.__open, null, 2000);
    const op = await page.evaluate(() => window.__open);
    assert.deepEqual([op.content, op.vis, op.pe, op.hit, op.mo], [true, 'visible', 'auto', true, 'speed'], 'menu usable on its first frame ' + JSON.stringify(op));
    assert.ok(op.op < 1, 'the first open eases in (opacity ' + op.op + ')');
    await page.waitForTimeout(300);
    assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-menu')).opacity), '1');
    if (process.env.YTC_SHOTS) for (const [sel, name] of [['#ytc-speed', 'speed'], ['#ytc-qual', 'quality'], ['#ytc-ccb', 'cc']]) {
      const [x, y] = await center(sel); await page.mouse.move(x, y); await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `player-menu-${name}.png`), clip: { x: pl.x + pl.width - 380, y: pl.y + pl.height - 420, width: 380, height: 420 } });
    }
    if (process.env.YTC_SHOTS) { await page.mouse.move(sx, sy); await page.waitForTimeout(100); }
    // 2. Sweep Speed → Quality → CC along the toolbar: every frame shows exactly one menu, never none, never re-fading.
    await page.evaluate(() => {
      const box = document.getElementById('ytc-player'), m = document.getElementById('ytc-menu'), w = window;
      w.__sweep = { frames: 0, closed: 0, multi: 0, minOp: 1, kinds: [] }; w.__sweeping = true;
      const f = () => {
        if (!w.__sweeping) return;
        const k = ['msp', 'mq', 'mcc'].filter(c => m.querySelector('.' + c));
        const s = w.__sweep; s.frames++;
        if (!box.classList.contains('menu')) s.closed++;
        if (k.length !== 1) s.multi++;
        s.minOp = Math.min(s.minOp, +getComputedStyle(m).opacity);
        if (k[0] && s.kinds[s.kinds.length - 1] !== k[0]) s.kinds.push(k[0]);
        requestAnimationFrame(f);
      };
      requestAnimationFrame(f);
    });
    const [qx, qy] = await center('#ytc-qual'), [cx, cy] = await center('#ytc-ccb');
    await page.mouse.move(qx, qy, { steps: 4 });
    await page.waitForTimeout(60);
    await page.mouse.move(cx, cy, { steps: 6 });
    await page.waitForTimeout(150);
    await page.mouse.move(qx, qy, { steps: 6 });
    await page.mouse.move(sx, sy, { steps: 6 });
    await page.waitForTimeout(120);
    const sw = await page.evaluate(() => { window.__sweeping = false; return window.__sweep; });
    assert.equal(sw.closed, 0, 'never no menu while sweeping ' + JSON.stringify(sw));
    assert.equal(sw.multi, 0, 'always exactly one menu ' + JSON.stringify(sw));
    assert.equal(sw.minOp, 1, 'sweeping does not re-animate ' + JSON.stringify(sw));
    // Toolbar order is CC · Speed · Quality, so Quality → CC crosses Speed (16 ms intent opens it on the way).
    assert.ok(sw.kinds[1] === 'mq' && sw.kinds.includes('mcc'), 'each shortcut opened its menu ' + JSON.stringify(sw));
    assert.equal(sw.kinds[sw.kinds.length - 1], 'msp', 'ends on the menu under the pointer ' + JSON.stringify(sw));
    // Leaving closes it (grace), fading out with its content, then inert.
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3, { steps: 4 });
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-menu')).pointerEvents), 'none', 'a closing menu takes no clicks');
    // 3. Idle: the still pointer lets the bar fade (eased), any move brings it back on the same frame.
    await waitT(() => document.getElementById('ytc-player').classList.contains('idle'), null, 4000);
    await page.waitForTimeout(80);
    const mid = +(await page.evaluate(() => getComputedStyle(document.querySelector('.pctl')).opacity));
    assert.ok(mid > 0 && mid < 1, 'idle hide eases out (opacity ' + mid + ' 80 ms in)');
    await page.waitForTimeout(600);
    assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('.pctl')).opacity), '0');
    const wake = () => page.evaluate(() => new Promise(res => {
      const box = document.getElementById('ytc-player');
      const mo = new MutationObserver(() => { if (!box.classList.contains('idle')) { mo.disconnect(); res(getComputedStyle(document.querySelector('.pctl')).opacity); } });
      mo.observe(box, { attributes: true, attributeFilter: ['class'] });
    }));
    let woke = wake();
    await page.mouse.move(pl.x + pl.width / 2 + 30, pl.y + pl.height / 3);
    assert.equal(await woke, '1', 'wake-up is instant');
    // 4. Leaving the player while playing hides the bar at once; moving elsewhere on the page does not wake it; coming back does.
    await page.mouse.move(5, 5);
    await waitT(() => document.getElementById('ytc-player').classList.contains('idle'), null, 500);
    await page.mouse.move(40, 8);
    await page.waitForTimeout(250);
    assert.ok(await page.evaluate(() => document.getElementById('ytc-player').classList.contains('idle')), 'moves outside the player do not wake it');
    woke = wake();
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    assert.equal(await woke, '1', 'back over the player: shown at once');
    // 5. Keyboard feedback appears on the key's frame (no fade-in).
    const toast = page.evaluate(() => new Promise(res => {
      const box = document.getElementById('ytc-player');
      const mo = new MutationObserver(() => { if (box.classList.contains('toast')) { mo.disconnect(); res([getComputedStyle(document.getElementById('ytc-toast')).opacity, document.getElementById('ytc-toast').textContent]); } });
      mo.observe(box, { attributes: true, attributeFilter: ['class'] });
    }));
    await keys('l');
    assert.deepEqual(await toast, ['1', '+10 s']);
    // Frames: the same motion again with nothing but pointer moves and waits (no probes forcing style or layout):
    // menu open, sweep, close, idle fade and wake must produce no long tasks. (The largest frame gap is reported
    // only: headless Chromium paces rAF at 30–120 Hz and drops a frame now and then with nothing running.)
    await page.evaluate(() => {
      const w = window; w.__lt = []; w.__gap = 0; w.__probe = true;
      new PerformanceObserver(l => { if (w.__probe) w.__lt.push(...l.getEntries().map(e => Math.round(e.duration))); }).observe({ type: 'longtask' });
      let last = performance.now(); const f = t => { if (w.__probe) { w.__gap = Math.max(w.__gap, t - last); last = t; requestAnimationFrame(f); } }; requestAnimationFrame(f);
    });
    await page.mouse.move(sx, sy, { steps: 3 });
    await page.waitForTimeout(250);
    await page.mouse.move(qx, qy, { steps: 4 });
    await page.mouse.move(cx, cy, { steps: 6 });
    await page.waitForTimeout(150);
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3, { steps: 4 });
    await page.waitForTimeout(3600); // grace close, then idle (2.5 s) and its fade
    await page.mouse.move(pl.x + pl.width / 2 + 20, pl.y + pl.height / 3);
    await page.waitForTimeout(200);
    const probe = await page.evaluate(() => { window.__probe = false; return { lt: window.__lt, gap: Math.round(window.__gap), idle: document.getElementById('ytc-player').className }; });
    assert.deepEqual(probe.lt, [], 'no long tasks during menu open, sweep, close, idle fade and wake');
    console.log(`     motion frames: largest gap between animation frames ${probe.gap} ms, long tasks ${probe.lt.length}`);
    // 6. Progress: while playing the fill sits on the true position (glides, no lag); a scrub lands 1:1 on its frame.
    await page.waitForTimeout(400);
    const track = await page.evaluate(() => {
      const v = document.getElementById('ytc-video'), m = new DOMMatrix(getComputedStyle(document.querySelector('.ppos')).transform);
      return { shown: m.a * v.duration, t: v.currentTime, glide: document.getElementById('ytc-player').classList.contains('glide') };
    });
    assert.ok(track.glide && Math.abs(track.shown - track.t) < 0.35, 'the fill tracks playback ' + JSON.stringify(track));
    const bar = await page.locator('#ytc-seek').boundingBox();
    await page.mouse.move(bar.x + bar.width * 0.3, bar.y + bar.height / 2);
    await page.mouse.down();
    await page.mouse.move(bar.x + bar.width * 0.6, bar.y + bar.height / 2);
    const scrub = await page.evaluate(() => {
      const ppos = new DOMMatrix(getComputedStyle(document.querySelector('.ppos')).transform).a;
      const knob = document.querySelector('.pknob').getBoundingClientRect(), seek = document.getElementById('ytc-seek').getBoundingClientRect();
      return { ppos, knob: (knob.x - seek.x) / seek.width, drag: document.getElementById('ytc-player').classList.contains('drag') };
    });
    await page.mouse.up();
    assert.ok(scrub.drag && Math.abs(scrub.ppos - 0.6) < 0.01 && Math.abs(scrub.knob - 0.6) < 0.01, 'scrub tracks the pointer 1:1 ' + JSON.stringify(scrub));
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime / document.getElementById('ytc-video').duration - 0.6) < 0.02, null, 3000);
  });

  await step('player morphs: play/pause icon morphs on a pointer click and flips at once on Space; mute/CC/theater morph (keys instant); menu panel morphs between menus, clickable mid-morph; selection mark slides (keys jump); center pulse; seek ripple', async () => {
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await waitT(() => { const v = document.getElementById('ytc-video'); return !v.paused && v.currentTime > 0.3; }, null, 8000);
    const pl = await page.locator('#ytc-player').boundingBox();
    const center = async sel => { const b = await page.locator(sel).boundingBox(); return [b.x + b.width / 2, b.y + b.height / 2]; };
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    const slow = async rate => { await cdp.send('Animation.enable'); await cdp.send('Animation.setPlaybackRate', { playbackRate: rate }); };
    const shots = process.env.YTC_SHOTS;
    const midShot = async (name, clip) => { if (shots) await page.screenshot({ path: path.join(shots, name + '.png'), ...(clip ? { clip } : {}) }); };
    // Every icon in the control bar sits on its button's centre (±1 px), in both states.
    const centred = () => page.evaluate(() => [...document.querySelectorAll('.prow .pb')].filter(b => b.offsetParent).flatMap(b => [...b.querySelectorAll('svg')].map(i => {
      const a = b.getBoundingClientRect(), r = i.getBoundingClientRect(); return { b: b.getAttribute('aria-label'), dx: +((r.left + r.width / 2) - (a.left + a.width / 2)).toFixed(1), dy: +((r.top + r.height / 2) - (a.top + a.height / 2)).toFixed(1) };
    })).filter(o => Math.abs(o.dx) > 1 || Math.abs(o.dy) > 1));
    assert.deepEqual(await centred(), [], 'player icons centred (playing)');
    // A state flip observed on the frame after the class changed: a pointer flip is mid-morph (a running `d`
    // transition, the shape between the two), a key flip is already final.
    const flip = (sel, cls) => page.evaluate(([sel, cls]) => new Promise(res => {
      const box = document.getElementById('ytc-player'), w = document.getElementById('ytc-watch'), el = document.querySelector(sel);
      const on = () => cls === 'theater' ? w.classList.contains('theater') : box.classList.contains(cls), was = on();
      const mo = new MutationObserver(() => {
        if (on() === was) return;
        mo.disconnect();
        requestAnimationFrame(() => requestAnimationFrame(() => {
          const mid = getComputedStyle(el).d, run = el.getAnimations().map(a => a.transitionProperty).filter(Boolean), kf = box.classList.contains('kf');
          setTimeout(() => res({ mid, end: getComputedStyle(el).d, run, kf }), 500);
        }));
      });
      mo.observe(box, { attributes: true, attributeFilter: ['class'] }); mo.observe(w, { attributes: true, attributeFilter: ['class'] });
      setTimeout(() => res(null), 3000);
    }), [sel, cls]);
    const P1 = '.prow .i-pp .p1';
    let f = flip(P1, 'paused');
    await page.click('.prow [data-act=p-toggle]');
    let r = await f;
    assert.ok(r && r.run.includes('d') && r.mid !== r.end && !r.kf, 'pointer pause: the icon morphs ' + JSON.stringify(r));
    assert.deepEqual(await centred(), [], 'player icons centred (paused)');
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    f = flip(P1, 'paused');
    await page.keyboard.press(' ');
    r = await f;
    assert.ok(r && r.run.length === 0 && r.mid === r.end && r.kf, 'Space: the icon flips at once ' + JSON.stringify(r));
    // Mid-morph look (slowed 10×): play → pause.
    await slow(0.1);
    await page.click('.prow [data-act=p-toggle]');
    await page.waitForTimeout(700);
    await midShot('morph-playpause-mid', { x: pl.x, y: pl.y + pl.height - 56, width: 200, height: 56 });
    await slow(1);
    await page.waitForTimeout(400);
    if (await page.evaluate(() => document.getElementById('ytc-video').paused)) await page.keyboard.press('k');
    // Mute: the cross turns in and the waves leave (pointer); m flips at once.
    f = flip('.i-vm .vx', 'muted');
    await page.click('.prow [data-act=p-mute]');
    r = await page.evaluate(() => new Promise(res => requestAnimationFrame(() => res(document.querySelector('.i-vm .vx').getAnimations().map(a => a.transitionProperty)))));
    await f;
    assert.ok(r.includes('transform') || r.includes('opacity'), 'pointer mute morphs the icon ' + JSON.stringify(r));
    await page.keyboard.press('m');
    r = await page.evaluate(() => new Promise(res => requestAnimationFrame(() => res(document.querySelector('.i-vm .vx').getAnimations().length))));
    assert.equal(r, 0, 'm: instant');
    // Theater: the frame changes shape on a pointer click; t is instant.
    f = flip('.i-th path', 'theater');
    await page.click('.prow [data-act=p-theater]');
    r = await f;
    assert.ok(r && r.run.includes('d') && r.mid !== r.end, 'pointer theater: the frame morphs ' + JSON.stringify(r));
    f = flip('.i-th path', 'theater');
    await page.keyboard.press('t');
    r = await f;
    assert.ok(r && r.run.length === 0 && r.mid === r.end, 't: instant ' + JSON.stringify(r));
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    // Menus: Speed open, then Quality: the panel keeps its opacity and morphs its surface (a clip on its ::before
    // between the two heights) while the new menu is in the DOM and its bottom row takes clicks on that frame.
    const [sx, sy] = await center('#ytc-speed');
    await page.mouse.move(sx, sy, { steps: 3 });
    await page.waitForSelector('#ytc-player.menu #ytc-menu .msp');
    await page.waitForTimeout(350);
    const h0 = await page.evaluate(() => document.getElementById('ytc-menu').getBoundingClientRect().height);
    const probe = page.evaluate(() => new Promise(res => {
      const m = document.getElementById('ytc-menu');
      const mo = new MutationObserver(() => {
        if (!m.querySelector('.mq')) return;
        mo.disconnect();
        const now = () => {
          const b = [...m.querySelectorAll('#ytc-mq button')].pop(), q = b.getBoundingClientRect(), hit = document.elementFromPoint(q.x + q.width / 2, q.y + q.height / 2);
          const surf = document.getAnimations().filter(a => a.effect && a.effect.target === m && a.effect.pseudoElement === '::before');
          return { clip: getComputedStyle(m, '::before').clipPath, mx: getComputedStyle(m).getPropertyValue('--mx'), h: m.getBoundingClientRect().height, op: getComputedStyle(m).opacity, hit: !!hit && (hit === b || b.contains(hit)), surf: surf.length };
        };
        const first = now();
        requestAnimationFrame(() => requestAnimationFrame(() => res({ first, next: now() })));
      });
      mo.observe(m, { childList: true, subtree: true });
      setTimeout(() => res(null), 2000);
    }));
    const [qx, qy] = await center('#ytc-qual');
    await page.mouse.move(qx, qy, { steps: 2 });
    const mp = await probe;
    assert.ok(mp, 'quality menu opened');
    assert.ok(mp.first.surf === 1 && mp.first.hit && mp.first.op === '1', 'the switch morphs the panel, content clickable on its frame ' + JSON.stringify(mp));
    // The surface (::before) is the panel's box extended by --mx (a shrinking morph still paints the old box) and
    // clipped by inset(top right bottom left): painted height = box + top/bottom extension − top/bottom clip.
    const ins = c => { const v = (/inset\(([^)]*?)(?: round [^)]*)?\)/.exec(c) || [0, '0'])[1].trim().split(/\s+/).map(parseFloat); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; };
    const ext = mx => { const v = (mx || '0').trim().split(/\s+/).map(x => -parseFloat(x) || 0); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; };
    const [ct, , cb] = ins(mp.next.clip), [et, , eb] = ext(mp.next.mx);
    const painted = mp.next.h + et + eb - ct - cb;
    assert.ok(Math.abs(mp.next.h - h0) > 4 && Math.min(h0, mp.next.h) - 1 <= painted && painted <= Math.max(h0, mp.next.h) + 1 && Math.abs(painted - mp.next.h) > 0.2,
      `the surface is between the two menus' heights mid-morph (old ${h0}, new ${mp.next.h}, painted ${painted}) ` + JSON.stringify(mp));
    await page.waitForTimeout(450);
    assert.equal(await page.evaluate(() => document.getElementById('ytc-menu').getAnimations({ subtree: true }).length + document.getAnimations().filter(a => a.effect && a.effect.pseudoElement === '::before' && a.effect.target.id === 'ytc-menu').length), 0, 'morph finished, nothing left running');
    // Mid-morph look (slowed): Quality → Speed.
    await slow(0.1);
    await page.mouse.move(sx, sy, { steps: 2 });
    await page.waitForTimeout(900);
    await midShot('morph-menu-mid', { x: pl.x + pl.width - 380, y: pl.y + pl.height - 420, width: 380, height: 420 });
    await slow(1);
    await page.waitForTimeout(500);
    // Selection mark (Speed presets): a pointer choice springs the tint from 1.0 to 1.5; a key jumps it.
    await page.locator('#ytc-mpre button', { hasText: /^1\.5×$/ }).click();
    const mk = await page.evaluate(() => new Promise(res => requestAnimationFrame(() => requestAnimationFrame(() => { const m = document.querySelector('#ytc-mpre > i.pmk'); res({ ok: !!m, run: m ? m.getAnimations().map(a => a.transitionProperty) : [] }); }))));
    assert.ok(mk.ok && mk.run.includes('clip-path'), 'the selection mark slides ' + JSON.stringify(mk));
    await page.waitForTimeout(450);
    const markOn = () => page.evaluate(() => { const m = document.querySelector('#ytc-mpre > i.pmk'), b = document.querySelector('#ytc-mpre button.on'), s = document.getElementById('ytc-mpre').getBoundingClientRect(), r = b.getBoundingClientRect(), v = (c => { const v = /inset\(([^)]*?)(?: round [^)]*)?\)/.exec(c)[1].trim().split(/\s+/).map(parseFloat); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; })(getComputedStyle(m).clipPath); return { dl: v[3] - (r.left - s.left), dt: v[0] - (r.top - s.top), dw: (s.width - v[1] - v[3]) - r.width, label: b.textContent, run: m.getAnimations().length }; });
    let mo = await markOn();
    assert.ok(Math.abs(mo.dl) < 0.6 && Math.abs(mo.dt) < 0.6 && Math.abs(mo.dw) < 0.6 && mo.label === '1.5×', 'the mark lands exactly on 1.5 ' + JSON.stringify(mo));
    await page.keyboard.press('>');
    mo = await page.evaluate(() => new Promise(res => requestAnimationFrame(() => requestAnimationFrame(() => res(document.querySelector('#ytc-mpre > i.pmk').getAnimations().length)))));
    assert.equal(mo, 0, 'a key moves the mark at once');
    mo = await markOn();
    assert.ok(Math.abs(mo.dl) < 0.6 && mo.label === '1.75×', 'the mark follows the key ' + JSON.stringify(mo));
    await page.locator('#ytc-mpre button', { hasText: /^1×$/ }).click();
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3, { steps: 3 });
    await page.waitForSelector('#ytc-player:not(.menu)', { state: 'attached', timeout: 1500 });
    // Center pulse: a click on the video leaves the action's icon growing and fading in the middle; Space does not.
    await page.waitForTimeout(300);
    await page.mouse.click(pl.x + pl.width / 2, pl.y + pl.height / 3);
    let pu = await page.evaluate(() => { const e = document.getElementById('ytc-pfx'); return { cls: e.className, run: e.getAnimations().map(a => a.animationName + ':' + a.playState) }; });
    assert.ok(/\bon\b/.test(pu.cls) && /\bpa\b/.test(pu.cls) && pu.run.includes('ytcpfx:running'), 'pause by click: pulse ' + JSON.stringify(pu));
    await page.waitForTimeout(700);
    await page.keyboard.press('k');
    pu = await page.evaluate(() => { const e = document.getElementById('ytc-pfx'); return { cls: e.className, run: e.getAnimations().map(a => a.animationName + ':' + a.playState) }; });
    assert.ok(/\bpa\b/.test(pu.cls) && !pu.run.includes('ytcpfx:running'), 'k: no pulse ' + JSON.stringify(pu));
    await slow(0.1);
    await page.mouse.click(pl.x + pl.width / 2, pl.y + pl.height / 3);
    await page.waitForTimeout(1200);
    await midShot('pulse-mid', { x: pl.x + pl.width / 2 - 120, y: pl.y + pl.height / 2 - 120, width: 240, height: 240 });
    await slow(1);
    await page.waitForTimeout(400);
    if (await page.evaluate(() => document.getElementById('ytc-video').paused)) await page.keyboard.press('k');
    // Seek ripple: pressing the bar swells a mint dot where it landed (on pointerdown).
    const bar = await page.locator('#ytc-seek').boundingBox();
    await page.mouse.move(bar.x + bar.width * 0.4, bar.y + bar.height / 2);
    await page.mouse.down();
    const rp = await page.evaluate(() => { const e = document.getElementById('ytc-rip'), s = document.getElementById('ytc-seek'); return { run: e.getAnimations().map(a => a.animationName), r: +s.style.getPropertyValue('--r') }; });
    await page.mouse.up();
    assert.ok(rp.run.includes('ytcrip') && Math.abs(rp.r - 0.4) < 0.01, 'press ripple at the pointer ' + JSON.stringify(rp));
    // Captions mid-wipe (slowed): turning them on.
    await slow(0.1);
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3);
    await page.keyboard.press('c'); // (instant: the key)
    await page.keyboard.press('c');
    await page.waitForTimeout(500);
    await page.evaluate(() => scrollTo(0, 0));
    await page.click('#ytc-ccb');
    await page.click('#ytc-menu .msw');
    await page.waitForTimeout(900);
    { const b = await page.locator('#ytc-player').boundingBox(); await midShot('morph-cc-mid', { x: b.x + b.width - 300, y: b.y + b.height - 56, width: 300, height: 56 }); }
    await slow(1);
    await page.waitForTimeout(500);
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3, { steps: 3 });
    await page.keyboard.press('Escape');
    if (await page.evaluate(() => document.getElementById('ytc-player').classList.contains('cc'))) await page.keyboard.press('c');
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => document.getElementById('ytc-player').classList.contains('cc')), false, 'captions left off');
    for (const w of [1400, 2560]) {
      await page.setViewportSize({ width: w, height: w === 2560 ? 1440 : 900 });
      await page.waitForTimeout(300);
      const b = await page.locator('#ytc-player').boundingBox();
      await page.mouse.move(b.x + b.width / 2, b.y + b.height / 3);
      await page.evaluate(() => document.activeElement && document.activeElement.blur());
      await page.waitForTimeout(200);
      assert.deepEqual(await centred(), [], `player icons centred at ${w}px`);
      await midShot(`player-bar-${w}`, { x: b.x, y: b.y + b.height - 70, width: b.width, height: 70 });
    }
    await page.setViewportSize({ width: 1400, height: 900 });
  });

  // ---- SponsorBlock (player/sponsor.rs, player/own/sb.rs)
  const sbCfg = async () => (await sw.evaluate(() => chrome.storage.local.get('ytcConfigV1'))).ytcConfigV1?.p || {};
  // Every played position (each animation frame) from now on, to prove no frame inside a skipped range played.
  const sbRec = () => page.evaluate(() => { const v = document.getElementById('ytc-video'); window.__sbT = []; const f = () => { window.__sbT.push(v.currentTime); window.__sbRaf = requestAnimationFrame(f); }; cancelAnimationFrame(window.__sbRaf); f(); });
  const sbSeen = () => page.evaluate(() => window.__sbT);
  await step('SponsorBlock: the lookup sends only the sha256 prefix (no cookies/referrer) in parallel with the player; the first frame does not wait; ranges fade in on the bar', async () => {
    SB.log.length = 0; SB.delay = 1500;
    await page.goto(`https://www.youtube.com/watch?v=${SB_ID}`, { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    const ffAt = await page.evaluate(() => performance.timeOrigin + +document.documentElement.dataset.ytcFirstFrame);
    // Caught on the frame the ranges arrive: they are fading in (an opacity transition from @starting-style).
    const fade = await (await page.waitForFunction(() => { const e = document.querySelector('#ytc-segs .psb'); return e && { run: e.getAnimations().map(a => a.transitionProperty).join() }; }, null, { polling: 'raf', timeout: 6000 })).jsonValue();
    SB.delay = 0;
    assert.equal(SB.log.length, 1, 'one lookup');
    const r = SB.log[0], u = new URL(r.url);
    assert.equal(u.pathname, '/api/skipSegments/' + sha4(SB_ID), 'only the 4-hex prefix of sha256(id)');
    assert.equal(sha4(SB_ID), '671e');
    assert.ok(!r.url.includes(SB_ID) && !r.url.includes('sponsorB'), 'the video id never leaves: ' + r.url);
    assert.ok(!r.headers.cookie && !r.headers.referer, 'no cookies, no referrer ' + JSON.stringify(r.headers));
    assert.deepEqual(JSON.parse(u.searchParams.get('categories')), ['sponsor', 'selfpromo', 'interaction', 'intro', 'outro', 'preview', 'music_offtopic', 'filler', 'poi_highlight'], 'every category asked (the answer does not reveal the choice)');
    assert.ok(r.sent < ffAt && ffAt < r.done, `sent with the player (${ffAt - r.sent} ms before the first frame), first frame ${r.done - ffAt} ms before the answer`);
    console.log(`     SponsorBlock lookup sent ${(ffAt - r.sent).toFixed(0)} ms before the first frame; the first frame did not wait for its 1.5 s answer`);
    assert.ok(fade.run.includes('opacity'), 'the ranges fade in ' + JSON.stringify(fade));
    // Four ranges (decoys from the same bucket are not ours), the highlight tick; sponsor at 8/40 of the bar.
    assert.equal(await page.locator('#ytc-segs .psb').count(), 4);
    assert.equal(await page.locator('#ytc-segs .psbp').count(), 1);
    const g = await page.evaluate(() => { const b = document.getElementById('ytc-segs').getBoundingClientRect(); return [...document.querySelectorAll('#ytc-segs .psb,#ytc-segs .psbp')].map(e => { const r = e.getBoundingClientRect(); return { c: e.className, l: (r.left - b.left) / b.width, r: (r.right - b.left) / b.width }; }); });
    const sp = g.find(x => x.c === 'psb k0');
    assert.ok(Math.abs(sp.l - 8 / 40) < 0.003 && Math.abs(sp.r - 14 / 40) < 0.003, 'sponsor range 8–14 s ' + JSON.stringify(g));
    assert.ok(Math.abs((g.find(x => x.c === 'psbp k8').l + g.find(x => x.c === 'psbp k8').r) / 2 - 34 / 40) < 0.003, 'highlight tick at 34 s');
    // The highlight is offered as a jump.
    await waitT(() => document.getElementById('ytc-sbt').classList.contains('on'));
    assert.equal(await page.textContent('#ytc-sbtl'), 'Highlight at 0:34');
    assert.equal(await page.textContent('#ytc-sbtb'), 'Jump');
    if (process.env.YTC_SHOTS) { await page.mouse.move(700, 400); await page.waitForTimeout(400); const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'sponsor-bar.png'), clip: { x: b.x, y: b.y + b.height - 120, width: b.width, height: 120 } }); }
    await page.click('#ytc-sbtb');
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime - 34) < 0.6);
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    assert.ok(!(await page.evaluate(() => document.getElementById('ytc-sbt').classList.contains('on'))), 'the toast leaves after Jump');
  });

  await step('SponsorBlock: playing into a sponsor skips it at its start (touching interaction too) with "Skipped sponsor · Undo"; Undo goes back and plays it', async () => {
    await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.currentTime = 5; v.playbackRate = 2; v.play(); });
    await page.waitForTimeout(150);
    await sbRec();
    await waitT(() => document.getElementById('ytc-sbt').classList.contains('on') && document.getElementById('ytc-sbtl').textContent.startsWith('Skipped'), null, 8000);
    await page.waitForTimeout(300);
    const seen = await sbSeen();
    const inside = seen.filter(t => t > 8.15 && t < 15.4);
    assert.deepEqual(inside, [], 'no frame inside the skipped 8–15.5 s');
    assert.ok(Math.max(...seen.filter(t => t < 8.5)) >= 7.5 && seen.some(t => t >= 15.5), 'played up to the sponsor, landed at 15.5 ' + JSON.stringify(seen.filter(t => t > 7 && t < 16.5).map(t => +t.toFixed(2))));
    console.log(`     SponsorBlock skip at ${Math.max(...seen.filter(t => t < 8.5)).toFixed(3)} s (segment starts at 8, tolerance 0.1; 2× speed)`);
    assert.equal(await page.textContent('#ytc-sbtl'), 'Skipped sponsor');
    assert.equal(await page.textContent('#ytc-sbtb'), 'Undo');
    assert.ok((await page.getAttribute('#ytc-sbt', 'class')).includes('k0'), 'sponsor color dot');
    if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'sponsor-toast.png'), clip: { x: b.x, y: b.y + b.height - 130, width: 420, height: 130 } }); }
    await page.click('#ytc-sbtb');
    await waitT(() => { const t = document.getElementById('ytc-video').currentTime; return t > 7.9 && t < 9; });
    assert.ok(!(await page.evaluate(() => document.getElementById('ytc-sbt').classList.contains('on'))), 'the toast leaves on Undo');
    await sbRec();
    await waitT(() => document.getElementById('ytc-video').currentTime > 15.8, null, 8000);
    const again = await sbSeen();
    const gaps = again.slice(1).map((t, i) => t - again[i]);
    assert.ok(Math.max(...gaps) < 0.6 && again.some(t => t > 11 && t < 13), 'after Undo the sponsor plays through (no jump) ' + Math.max(...gaps));
    assert.ok(!(await page.evaluate(() => document.getElementById('ytc-sbt').classList.contains('on'))), 'and is not skipped again');
  });

  await step('SponsorBlock: a manual seek into a segment only shows it; the tooltip names the category', async () => {
    const box = await page.locator('.pseek').boundingBox();
    await page.mouse.move(box.x + box.width * 27 / 40, box.y + box.height / 2, { steps: 4 });
    await waitT(() => document.getElementById('ytc-tipk').textContent === 'Self-promo');
    assert.equal(await page.getAttribute('#ytc-tipk', 'class'), 'ptk k1');
    if (process.env.YTC_SHOTS) { await page.waitForTimeout(600); const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'sponsor-tooltip.png'), clip: { x: b.x, y: b.y + b.height - 120, width: b.width, height: 120 } }); }
    await page.mouse.click(box.x + box.width * 27 / 40, box.y + box.height / 2);
    await sbRec();
    await page.waitForTimeout(900);
    const seen = await sbSeen();
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    assert.ok(seen.length > 10 && seen.every(t => t > 26.5 && t < 30), 'stayed inside self-promo 26–30 s ' + JSON.stringify([seen[0], seen.at(-1)]));
    assert.ok(seen.at(-1) - seen[0] > 0.8, 'and played');
    assert.ok(!(await page.evaluate(() => document.getElementById('ytc-sbt').classList.contains('on'))), 'no skip toast');
    await page.mouse.move(700, 300);
  });

  await step('SponsorBlock: hover menu (categories, counts) remembered in ytcConfigV1; the switch turns lookups and skipping off', async () => {
    await page.evaluate(() => scrollTo(0, 0));
    const pl = await page.locator('#ytc-player').boundingBox();
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 2);
    await page.hover('#ytc-sbb');
    await waitT(() => document.getElementById('ytc-player').classList.contains('menu') && document.getElementById('ytc-menu').dataset.m === 'sb');
    const rows = await page.$$eval('#ytc-msbc button', bs => bs.map(b => [b.querySelector('span').textContent, b.classList.contains('on'), b.querySelector('small')?.textContent || '']));
    assert.deepEqual(rows, [['Sponsor', true, '1'], ['Self-promo', true, '1'], ['Interaction', true, '1'], ['Intro', false, '1'], ['Outro', false, ''], ['Preview', false, ''], ['Non-music', false, ''], ['Filler', false, '']]);
    assert.equal(await page.getAttribute('#ytc-msbsw', 'aria-checked'), 'true');
    await page.waitForTimeout(250);
    if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'sponsor-menu.png'), clip: { x: b.x + b.width - 460, y: b.y + b.height - 330, width: 460, height: 330 } }); }
    await page.click('#ytc-msbc [data-arg="sb:intro"]');
    assert.equal(await page.getAttribute('#ytc-msbc [data-arg="sb:intro"]', 'aria-pressed'), 'true');
    await page.waitForTimeout(500);
    assert.equal((await sbCfg()).sb, 'sponsor,selfpromo,interaction,intro', 'choice remembered');
    await page.click('#ytc-msbc [data-arg="sb:intro"]');
    await page.waitForTimeout(500);
    assert.equal((await sbCfg()).sb, undefined, 'the default is not stored');
    // Off: no ranges, no skip, stored; the icon gets its slash.
    await page.click('#ytc-msbsw');
    assert.equal(await page.getAttribute('#ytc-msbsw', 'aria-checked'), 'false');
    await waitT(() => document.getElementById('ytc-player').classList.contains('sboff') && !document.querySelector('#ytc-segs .psb'));
    await page.waitForTimeout(500);
    assert.equal((await sbCfg()).sbo, false, 'off remembered');
    if (process.env.YTC_SHOTS) { await page.waitForTimeout(400); const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'sponsor-menu-off.png'), clip: { x: b.x + b.width - 460, y: b.y + b.height - 330, width: 460, height: 330 } }); }
    await page.mouse.move(pl.x + pl.width / 2, pl.y + pl.height / 3, { steps: 3 });
    await waitT(() => !document.getElementById('ytc-player').classList.contains('menu'));
    await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.currentTime = 7; v.play(); });
    await sbRec();
    await waitT(() => document.getElementById('ytc-video').currentTime > 10, null, 8000);
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    const seen = await sbSeen();
    assert.ok(seen.some(t => t > 8.5 && t < 10), 'off: the sponsor plays');
    assert.ok(!(await page.evaluate(() => document.getElementById('ytc-sbt').classList.contains('on'))), 'off: no toast');
    // Off: another video sends no lookup at all.
    const n = SB.log.length;
    await page.goto(`https://www.youtube.com/watch?v=${SB_ID2}`, { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await page.waitForTimeout(500);
    assert.equal(SB.log.length, n, 'off: no request');
    assert.ok(await page.evaluate(() => document.getElementById('ytc-player').classList.contains('sboff')));
    // On again from the menu: the lookup goes out now.
    await page.hover('#ytc-sbb');
    await waitT(() => document.getElementById('ytc-menu').dataset.m === 'sb' && document.getElementById('ytc-player').classList.contains('menu'));
    await page.click('#ytc-msbsw');
    for (let i = 0; i < 30 && SB.log.length === n; i++) await page.waitForTimeout(100);
    assert.equal(SB.log.length, n + 1, 'on: looked up');
    assert.equal(new URL(SB.log.at(-1).url).pathname, '/api/skipSegments/' + sha4(SB_ID2));
    await page.waitForTimeout(500);
    assert.equal((await sbCfg()).sbo, undefined, 'on is the default (not stored)');
    assert.equal(await page.locator('#ytc-segs .psb').count(), 0, 'no ranges for a video the bucket does not list');
    await page.keyboard.press('Escape');
  });

  await step('own player: googlevideo 403 → one player refresh → plays', async () => {
    await page.goto('https://www.youtube.com/watch?v=refresh4033', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    for (let i = 0; i < 50 && !log.gv.some(g => g.v === 'refresh4033' && g.gen === 2 && g.a > 0); i++) await page.waitForTimeout(100);
    assert.equal(log.api.filter(a => a.ep === 'player' && a.body.videoId === 'refresh4033' && a.body.context.client.clientName === 'VISIONOS').length, 2);
    assert.ok(log.gv.some(g => g.v === 'refresh4033' && g.gen === 2 && g.a > 0));
    assert.ok(!page.url().includes('yt-native'));
  });

  // ---- Failures stay in the player: a panel in the
  // player box with the message and the reason, Try again (a new player, fresh VISIONOS) and Open on YouTube.
  const visCalls = (v, from = 0) => log.api.slice(from).filter(a => a.ep === 'player' && a.body.videoId === v && a.body.context.client.clientName === 'VISIONOS');
  const panel = async () => ({ url: new URL(page.url()).search, m: await page.textContent('#ytc-player.failed .pfm'), r: await page.textContent('#ytc-player.failed .pfr') });
  // The panel's buttons: label centred in its box (±0.5 px), both on one row, centred in the player (±1 px).
  const panelGeometry = () => page.evaluate(() => {
    const box = document.getElementById('ytc-player').getBoundingClientRect(), bs = [...document.querySelectorAll('#ytc-player .pfb .pfbt')];
    const off = bs.map(b => { const r = b.getBoundingClientRect(), g = document.createRange(); g.selectNodeContents(b); const t = g.getBoundingClientRect();
      return { b: b.textContent, dx: +((t.left + t.width / 2) - (r.left + r.width / 2)).toFixed(1), dy: +((t.top + t.height / 2) - (r.top + r.height / 2)).toFixed(1), top: r.top }; });
    const a = bs[0].getBoundingClientRect(), z = bs.at(-1).getBoundingClientRect(), p = document.querySelector('#ytc-player .pfail').getBoundingClientRect();
    return { off: off.filter(o => Math.abs(o.dx) > .5 || Math.abs(o.dy) > .5), row: Math.abs(off[0].top - off.at(-1).top), rowDx: +(((a.left + z.right) / 2) - (box.left + box.width / 2)).toFixed(1), panelDx: +(((p.left + p.right) / 2) - (box.left + box.width / 2)).toFixed(1), panelDy: +(((p.top + p.bottom) / 2) - (box.top + box.height / 2)).toFixed(1) };
  });
  await step('own player: forced failure (VISIONOS UNPLAYABLE) → the panel in the player (the page stays); reason in debug overlay', async () => {
    const docs = log.docs.length;
    await page.goto('https://www.youtube.com/watch?v=unplayable1&t=7', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-player.failed .pfail');
    await page.waitForTimeout(600);
    assert.deepEqual(await panel(), { url: '?v=unplayable1&t=7', m: "YouTube won't play this video here", r: 'VISIONOS UNPLAYABLE: Forced failure (e2e)' });
    assert.equal(log.docs.length, docs + 1, 'no navigation after the failure');
    assert.equal(await page.locator('#ytc-video').count(), 0, 'media released');
    assert.equal(await page.$eval('.pfr', e => getComputedStyle(e).userSelect), 'text', 'the reason is selectable');
    assert.deepEqual(await page.locator('#ytc-player .pfbt').allTextContents(), ['Try again', 'Open on YouTube']);
    for (const w of [1400, 2560]) {
      await page.setViewportSize({ width: w, height: w === 2560 ? 1440 : 900 });
      await page.waitForTimeout(300);
      const g = await panelGeometry();
      assert.deepEqual(g.off, [], `button labels centred at ${w}px`);
      assert.ok(g.row < .5 && Math.abs(g.rowDx) <= 1 && Math.abs(g.panelDx) <= 1 && Math.abs(g.panelDy) <= 1, `panel and buttons centred at ${w}px: ${JSON.stringify(g)}`);
      if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `player-failed-${w}.png`), clip: b }); }
    }
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.keyboard.press('Backquote');
    await page.waitForSelector('#ytc-debug.on');
    const dbg = await page.locator('#ytc-debug').textContent();
    assert.match(dbg, /player failed: unplayable1 at 7s \(panel shown\)/);
    assert.match(dbg, /player failed \d+ min ago · unplayable1 at 7s · VISIONOS UNPLAYABLE: Forced failure \(e2e\)/);
    await page.keyboard.press('Backquote');
  });

  await step('debug center: the forced failure arrived as a full incident report (kind player-failed)', async () => {
    for (let i = 0; i < 50 && !incidents.some(x => x.kind === 'player-failed' && /Forced failure/.test(x.reason)); i++) await page.waitForTimeout(100);
    const r = incidents.find(x => x.kind === 'player-failed' && /Forced failure/.test(x.reason));
    assert.ok(r, 'incident delivered: ' + JSON.stringify(incidents.map(x => x.kind)));
    assert.equal(r.player.video, 'unplayable1');
    assert.ok(Array.isArray(r.player.requests) && r.player.requests.some(x => x.includes('player VISIONOS')), JSON.stringify(r.player.requests));
    assert.ok(!JSON.stringify(r).includes('SAPISID') && !JSON.stringify(r).includes('sig='), 'no secrets in the report');
  });

  await step('failure panel: Try again (pointer and Enter) re-mounts the player with a fresh VISIONOS call; Open on YouTube goes native at the time', async () => {
    const docs = log.docs.length, inc0 = incidents.length, n0 = visCalls('unplayable1').length;
    await page.click('#ytc-pfretry');
    for (let i = 0; i < 50 && visCalls('unplayable1').length < n0 + 1; i++) await page.waitForTimeout(100);
    await page.waitForSelector('#ytc-player.failed .pfail');
    assert.equal(visCalls('unplayable1').length, n0 + 1, 'one fresh VISIONOS call');
    await page.focus('#ytc-pfretry');
    await page.keyboard.press('Enter');
    for (let i = 0; i < 50 && visCalls('unplayable1').length < n0 + 2; i++) await page.waitForTimeout(100);
    await page.waitForSelector('#ytc-player.failed .pfail');
    assert.equal(visCalls('unplayable1').length, n0 + 2, 'Enter on the focused button retries too');
    assert.equal(log.docs.length, docs, 'Try again stays in the page');
    for (let i = 0; i < 30 && incidents.slice(inc0).filter(x => x.kind === 'player-failed').length < 2; i++) await page.waitForTimeout(100);
    assert.equal(incidents.slice(inc0).filter(x => x.kind === 'player-failed').length, 2, 'each attempt reports its failure');
    nativeOk.push('https://www.youtube.com/watch?v=unplayable1&t=7s&yt-native=1');
    await page.locator('#ytc-player .pfbt', { hasText: 'Open on YouTube' }).click();
    await page.waitForURL(/yt-native=1/, { timeout: 8000 });
    assert.equal(new URL(page.url()).search, '?v=unplayable1&t=7s&yt-native=1');
    await page.waitForFunction(() => window.__nativeBundle === 1, null, { timeout: 5000 });
  });

  await step('failure panel: members-only and age-restricted show their message, without an incident', async () => {
    for (const [v, m] of [['members0001', 'Members-only video'], ['agerestrict', 'Age-restricted — sign-in needed']]) {
      const inc0 = incidents.length;
      await page.goto(`https://www.youtube.com/watch?v=${v}`, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-player.failed .pfail');
      assert.equal(await page.textContent('.pfm'), m);
      assert.equal(new URL(page.url()).search, `?v=${v}`);
      await page.waitForTimeout(300);
      assert.deepEqual(problems(inc0), [], 'expected, not an incident');
    }
  });

  // ---- Bot check recovery (2 Nov 2026): the other routes in parallel, the winner kept for the session.
  // SPA to the first related video: returns its id once its media is fetched.
  const relatedPlay = async () => {
    const card = page.locator('#ytc-list .c').filter({ has: page.locator('a.th[href^="/watch"]') }).first();
    const v = new URL(await card.locator('a.th').getAttribute('href'), 'https://www.youtube.com').searchParams.get('v');
    const docs = log.docs.length;
    await card.locator('a.th').hover();
    await page.waitForTimeout(150);
    await card.locator('a.t').click();
    for (let i = 0; i < 80 && !log.gv.some(g => g.v === v && g.a > 0); i++) await page.waitForTimeout(100);
    assert.ok(log.gv.some(g => g.v === v && g.a > 0), 'the next video plays from googlevideo');
    assert.equal(log.docs.length, docs, 'SPA');
    assert.ok(!page.url().includes('yt-native'));
    return v;
  };
  await step('own player: bot check → fresh visitor retry plays; later videos and prefetches keep that visitor', async () => {
    FX.bot = 'visitor';
    const mark = log.api.length;
    try {
      await page.goto('https://www.youtube.com/watch?v=botfresh001', { waitUntil: 'commit' });
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      assert.ok(!page.url().includes('yt-native'));
      const vis = visCalls('botfresh001', mark);
      assert.equal(vis.length, 2, 'the bot-checked call and one retry');
      assert.notEqual(vis[0].body.context.client.visitorData, FRESH_VD);
      assert.equal(vis[1].body.context.client.visitorData, FRESH_VD);
      assert.equal(vis[1].headers['x-goog-visitor-id'], FRESH_VD);
      assert.ok(vis.every(a => !a.headers.authorization && !a.headers.cookie), 'anonymous');
      const ids = log.api.slice(mark).filter(a => a.ep === 'visitor_id');
      assert.equal(ids.length, 1, 'one visitor_id');
      assert.ok(!ids[0].headers.cookie && !ids[0].headers['x-goog-visitor-id'] && !ids[0].body.context.client.visitorData, 'a new anonymous visitor');
      // Later videos in this session: the fresh visitor, no new visitor_id, no bot check.
      const v = await relatedPlay();
      assert.ok(visCalls(v, mark).some(a => a.body.context.client.visitorData === FRESH_VD), 'the next video goes by the fresh visitor');
      // Calls sent after the recovery's answer (+ a few ms to apply it); earlier prefetches may still go anonymous,
      // and one the page sent just before the answer reached it can land here up to ~150 ms later under load.
      const late = log.api.slice(mark).filter(a => a.ep === 'player' && a.body.context.client.clientName === 'VISIONOS' && a.body.videoId !== 'botfresh001' && a.t > vis[1].done + 30);
      const settled = late.filter(a => a.t > vis[1].done + 150 || a.body.context.client.visitorData === FRESH_VD);
      assert.ok(settled.length && settled.every(a => a.body.context.client.visitorData === FRESH_VD), 'every later VISIONOS call (prefetches too) uses it: ' + late.map(a => a.body.videoId + (a.body.context.client.visitorData === FRESH_VD ? '' : ' (old)')));
      assert.equal(log.api.slice(mark).filter(a => a.ep === 'visitor_id').length, 1, 'no second visitor_id');
      await page.keyboard.press('Backquote');
      await page.waitForSelector('#ytc-debug.on');
      assert.match(await page.locator('#ytc-debug').textContent(), /player own \(VISIONOS, fresh visitor\)/);
      await page.keyboard.press('Backquote');
    } finally { FX.bot = null; }
  });

  await step('own player: bot check on anonymous and fresh visitor → the account session plays (signed in), and stays the route', async () => {
    const had = (await ctx.cookies('https://www.youtube.com')).find(c => c.name === 'SAPISID');
    await ctx.addCookies([{ name: 'SAPISID', value: 'e2e-sapisid', domain: '.youtube.com', path: '/', secure: true }]);
    FX.bot = 'account';
    const mark = log.api.length;
    try {
      await page.goto('https://www.youtube.com/watch?v=botacct0001', { waitUntil: 'commit' });
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      assert.ok(!page.url().includes('yt-native'));
      await page.waitForTimeout(200);
      const vis = visCalls('botacct0001', mark);
      // The fresh visitor's VISIONOS call only goes out if visitor_id answers before the account call.
      assert.ok(vis.length === 2 || vis.length === 3, `anonymous, then the account session (and maybe a fresh visitor): ${vis.length}`);
      assert.equal(log.api.slice(mark).filter(a => a.ep === 'visitor_id').length, 1, 'visitor_id in parallel with the account call');
      assert.ok(!vis[0].headers.authorization, 'first call anonymous');
      const acct = vis.filter(a => a.headers.authorization);
      assert.equal(acct.length, 1);
      assert.match(acct[0].headers.authorization, /^SAPISIDHASH \d+_[0-9a-f]{40}/);
      assert.equal(acct[0].headers['x-goog-authuser'], '0');
      assert.equal(acct[0].body.context.client.visitorData, vis[0].body.context.client.visitorData, 'the account call keeps our own visitor');
      assert.ok(vis.slice(1).every(a => a.headers.authorization || a.body.context.client.visitorData === FRESH_VD), 'only the two recovery routes');
      // A later answer from the other attempt is ignored: still playing.
      await page.waitForTimeout(300);
      assert.ok(!page.url().includes('yt-native'));
      const v = await relatedPlay();
      const late = log.api.slice(mark).filter(a => a.ep === 'player' && a.body.context.client.clientName === 'VISIONOS' && a.body.videoId !== 'botacct0001' && a.t > acct[0].done + 30);
      assert.ok(visCalls(v, mark).some(a => a.headers.authorization), 'the next video (prefetched or on click) goes by the account session');
      assert.ok(late.length && late.every(a => a.headers.authorization), 'every later VISIONOS call (prefetches too) is signed: ' + late.map(a => a.body.videoId));
      assert.equal(log.api.slice(mark).filter(a => a.ep === 'visitor_id').length, 1, 'no new recovery');
    } finally {
      FX.bot = null;
      if (had) await ctx.addCookies([had]); else await ctx.clearCookies({ name: 'SAPISID' });
    }
  });

  await step('own player: bot check on every route → the panel (the page stays); the incident lists every step tried; the next video tries the remembered route only; Try again recovers fully', async () => {
    const had = (await ctx.cookies('https://www.youtube.com')).find(c => c.name === 'SAPISID');
    await ctx.addCookies([{ name: 'SAPISID', value: 'e2e-sapisid', domain: '.youtube.com', path: '/', secure: true }]);
    FX.bot = 'all';
    const mark = log.api.length, inc0 = incidents.length;
    try {
      await page.goto('https://www.youtube.com/watch?v=botall00001&t=5', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-player.failed .pfail', { timeout: 8000 });
      const p = await panel();
      assert.equal(p.url, '?v=botall00001&t=5', 'the page stays');
      assert.equal(p.m, "YouTube asks to sign in to confirm you're not a bot");
      assert.match(p.r, /^VISIONOS LOGIN_REQUIRED: Sign in to confirm you’re not a bot · tried: anonymous \(bot check\), /);
      assert.equal(visCalls('botall00001', mark).length, 3, 'anonymous + account + fresh visitor');
      assert.equal(log.api.slice(mark).filter(a => a.ep === 'visitor_id').length, 1);
      let r;
      for (let i = 0; i < 50 && !(r = incidents.slice(inc0).find(x => x.kind === 'player-failed')); i++) await page.waitForTimeout(100);
      assert.ok(r, 'incident delivered');
      assert.match(r.reason, /^VISIONOS LOGIN_REQUIRED: Sign in to confirm you’re not a bot · tried: anonymous \(bot check\), /);
      for (const s of ['account session (bot check)', 'fresh visitor (bot check)']) assert.ok(r.reason.includes(s), r.reason);
      assert.ok(r.player.requests.some(x => x.includes('visitor_id')) && r.player.requests.some(x => x.includes('player VISIONOS (account session)')), JSON.stringify(r.player.requests));
      const str = JSON.stringify(r);
      assert.ok(!str.includes('e2e-sapisid') && !str.includes('SAPISIDHASH') && !str.includes(FRESH_VD), 'no cookie, auth or visitor in the report');
      // Was "VISIONOS bot check earlier: opening natively": now the next video (SPA, same session) tries the
      // remembered route once and, blocked again, shows the panel without the parallel recovery.
      const m2 = log.api.length;
      await page.evaluate(() => { history.pushState({}, '', '/watch?v=botall00002'); dispatchEvent(new PopStateEvent('popstate')); });
      await page.waitForSelector('#ytc-player.failed .pfail', { timeout: 8000 });
      assert.equal(visCalls('botall00002', m2).length, 1, 'the remembered route only');
      assert.equal(log.api.slice(m2).filter(a => a.ep === 'visitor_id').length, 0, 'no parallel recovery');
      assert.match(await page.textContent('.pfr'), /every route was bot-checked shortly before/);
      assert.equal(new URL(page.url()).search, '?v=botall00002');
      // Try again: the full recovery (anonymous + account + fresh visitor), still blocked → the panel.
      const m3 = log.api.length;
      await page.click('#ytc-pfretry');
      for (let i = 0; i < 50 && visCalls('botall00002', m3).length < 3; i++) await page.waitForTimeout(100);
      assert.equal(visCalls('botall00002', m3).length, 3, 'Try again lifts the block: every route again');
      await page.waitForSelector('#ytc-player.failed .pfail');
      // The bot check is gone: Try again plays.
      FX.bot = null;
      await page.click('#ytc-pfretry');
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      await page.waitForSelector('#ytc-video');
      assert.equal(new URL(page.url()).search, '?v=botall00002');
    } finally {
      FX.bot = null;
      if (had) await ctx.addCookies([had]); else await ctx.clearCookies({ name: 'SAPISID' });
    }
  });

  await step('own player: 403 again after the refresh → the panel at the current time; Try again resumes there with a fresh VISIONOS call (not the cached one)', async () => {
    failures.gone = false;
    await page.goto('https://www.youtube.com/watch?v=gone403gone', { waitUntil: 'commit' });
    await waitT(() => document.getElementById('ytc-video')?.currentTime >= 4, null, 9000);
    failures.gone = true;
    const docs = log.docs.length, inc0 = incidents.length;
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button', { hasText: '144p' }).click();
    await page.waitForSelector('#ytc-player.failed .pfail', { timeout: 8000 });
    const p = await panel();
    assert.equal(p.url, '?v=gone403gone', 'the page stays');
    assert.equal(p.m, "This video isn't loading");
    assert.match(p.r, /^googlevideo HTTP 403 after a player refresh/);
    for (let i = 0; i < 30 && !incidents.slice(inc0).some(x => x.kind === 'player-failed'); i++) await page.waitForTimeout(100);
    assert.ok(incidents.slice(inc0).some(x => x.kind === 'player-failed' && /HTTP 403/.test(x.reason)), 'incident sent');
    await page.keyboard.press('Backquote');
    const at = +(await page.locator('#ytc-debug').textContent()).match(/player failed: gone403gone at (\d+)s/)[1];
    await page.keyboard.press('Backquote');
    assert.ok(at >= 4, `failed at ${at}s`);
    failures.gone = false;
    const n0 = visCalls('gone403gone').length;
    await page.click('#ytc-pfretry');
    await waitT(t => { const v = document.getElementById('ytc-video'); return v && v.readyState >= 2 && v.currentTime >= t; }, at, 9000);
    assert.equal(visCalls('gone403gone').length, n0 + 1, 'a fresh VISIONOS call');
    assert.equal(await page.locator('#ytc-player.failed').count(), 0);
    assert.equal(log.docs.length, docs, 'no navigation');
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button', { hasText: 'Auto' }).click();
  });

  await step('own player: decodingInfo ruling out the format being loaded never strands it (incident tz8jz7IiQK0)', async () => {
    await page.evaluate(() => localStorage.setItem('ytc-test-caps', JSON.stringify({ 243: { s: false, e: false } })));
    try {
      await page.goto('https://www.youtube.com/watch?v=capschange1', { waitUntil: 'commit' });
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
      await waitT(() => document.getElementById('ytc-video')?.videoHeight === 240, null, 9000);
      assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    } finally {
      await page.evaluate(() => localStorage.removeItem('ytc-test-caps'));
    }
  });

  await step('own player: decodingInfo before any media starts from the smooth format, no chunks of the ruled-out one (incident v8avTs34rDU)', async () => {
    await page.evaluate(() => localStorage.setItem('ytc-test-caps', JSON.stringify({ 243: { s: false, e: false } })));
    try {
      await page.goto('https://www.youtube.com/watch?v=capsrestart', { waitUntil: 'commit' });
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
      await waitT(() => document.getElementById('ytc-video')?.videoHeight === 240, null, 9000);
      await page.waitForTimeout(800);
      assert.ok(!page.url().includes('yt-native'), 'no native fallback');
      assert.deepEqual(log.gv.filter(g => g.v === 'capsrestart' && g.itag === '243' && g.a > 0), [], 'no 243 chunks after its index');
    } finally {
      await page.evaluate(() => localStorage.removeItem('ytc-test-caps'));
    }
  });

  await step('own player: a MediaSource that opens without its sourceopen event still plays (incident T2TuUC2fd4A, background tab)', async () => {
    await page.evaluate(() => localStorage.setItem('ytc-test-nosourceopen', '1'));
    try {
      await page.goto('https://www.youtube.com/watch?v=nosrcopen01', { waitUntil: 'commit' });
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
      assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    } finally {
      await page.evaluate(() => localStorage.removeItem('ytc-test-nosourceopen'));
    }
  });

  await step('own player: googlevideo silent for 2.5 s → fresh URLs once, then plays (no fallback)', async () => {
    await page.goto('https://www.youtube.com/watch?v=gvhangfirst', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 12000);
    assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    const gens = [...new Set(log.gv.filter(g => g.v === 'gvhangfirst').map(g => g.gen))];
    assert.deepEqual(gens, [1, 2], 'asked with the first URLs, then the refreshed ones: ' + gens);
  });

  // ---- Stalls are not failures: "Still loading — retrying…" in the player, retries in place.
  const stuckShot = async name => {
    if (!process.env.YTC_SHOTS) return;
    const b = await page.locator('#ytc-player').boundingBox();
    await page.screenshot({ path: path.join(process.env.YTC_SHOTS, name + '.png'), clip: { x: b.x + b.width - 460, y: b.y, width: 460, height: 64 } });
    await page.screenshot({ path: path.join(process.env.YTC_SHOTS, name + '-player.png'), clip: b });
  };
  const incident = async (from, kind) => { let r; for (let i = 0; i < 50 && !(r = incidents.slice(from).find(x => x.kind === kind)); i++) await page.waitForTimeout(100); return r; };
  await step('own player: no first frame at 4 s → "Still loading — retrying…" in the player + a player-stuck incident; fresh URLs at 6 s; media arriving later recovers it (player-recovered)', async () => {
    const inc0 = incidents.length, docs = log.docs.length;
    STALL.hold.set('stallstart1', 'all');
    try {
      await page.goto('https://www.youtube.com/watch?v=stallstart1', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-player.stuck', { timeout: 9000 });
      await page.waitForTimeout(300);
      assert.ok(await page.locator('#ytc-pstuck').isVisible(), 'status visible');
      assert.equal(await page.textContent('#ytc-pstuck .pst'), 'Still loading — retrying…');
      assert.deepEqual(await page.locator('#ytc-pstuck .pfbt').allTextContents(), ['Try again', 'Open on YouTube']);
      // Non-blocking: the middle of the player is still the video's own surface.
      assert.equal(await page.evaluate(() => { const r = document.getElementById('ytc-player').getBoundingClientRect(); return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2).className; }), 'pov');
      const geo = await page.evaluate(() => [...document.querySelectorAll('#ytc-pstuck .pfbt')].map(b => { const r = b.getBoundingClientRect(), g = document.createRange(); g.selectNodeContents(b); const t = g.getBoundingClientRect(); return [+((t.left + t.width / 2) - (r.left + r.width / 2)).toFixed(1), +((t.top + t.height / 2) - (r.top + r.height / 2)).toFixed(1)]; }));
      assert.ok(geo.every(([dx, dy]) => Math.abs(dx) <= .5 && Math.abs(dy) <= .5), 'status button labels centred: ' + JSON.stringify(geo));
      await stuckShot('player-stuck-1400');
      const r = await incident(inc0, 'player-stuck');
      assert.ok(r, 'player-stuck incident');
      assert.equal(r.reason, 'no first frame 4 s after the streams were chosen: retrying in place');
      assert.ok(r.player && r.player.stuck && r.player.video === 'stallstart1', 'with the full player report');
      for (let i = 0; i < 60 && !log.gv.some(g => g.v === 'stallstart1' && g.gen === 3); i++) await page.waitForTimeout(100);
      assert.deepEqual([...new Set(log.gv.filter(g => g.v === 'stallstart1').map(g => g.gen))], [1, 2, 3], 'refresh at 2.5 s, fresh URLs at 6 s');
      assert.equal(await page.locator('#ytc-player.stuck').count(), 1, 'still stuck, still the player');
      unstall('stallstart1');
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      await waitT(() => !document.getElementById('ytc-player').classList.contains('stuck'), null, 2000);
      const ok = await incident(inc0, 'player-recovered');
      assert.ok(ok, 'player-recovered incident');
      assert.match(ok.reason, /^no first frame 4 s after the streams were chosen: recovered after \d+\.\d s \(1 retry with fresh URLs\)$/);
      assert.deepEqual(problems(inc0).map(x => x.split(':')[0]), ['player-stuck', 'player-recovered'], 'one episode, one outcome');
      assert.equal(log.docs.length, docs + 1, 'no navigation');
      assert.equal(new URL(page.url()).search, '?v=stallstart1');
      await waitT(() => { const v = document.getElementById('ytc-video'); return v && !v.paused && v.currentTime > 0.5; }, null, 6000);
    } finally { unstall('stallstart1'); }
  });

  await step('own player: index in but chunks stuck (data, no picture) → at 6 s a fresh MediaSource + fresh URLs; plays when they answer (2560 px)', async () => {
    const inc0 = incidents.length;
    STALL.hold.set('stalldata01', 'chunks');
    await page.setViewportSize({ width: 2560, height: 1440 });
    try {
      await page.goto('https://www.youtube.com/watch?v=stalldata01&t=10', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-player.stuck', { timeout: 9000 });
      await page.waitForTimeout(300);
      await stuckShot('player-stuck-2560');
      // gen 2: the throttle ladder (no headers on the URL's host, nor on its mirror → fresh URLs at ~1.9 s);
      // gen 3: the stuck retry at 6 s, which also rebuilds the MediaSource.
      for (let i = 0; i < 80 && !log.gv.some(g => g.v === 'stalldata01' && g.gen >= 3 && g.a > 0); i++) await page.waitForTimeout(100);
      assert.ok(log.gv.some(g => g.v === 'stalldata01' && g.gen >= 3 && g.a > 0), 'chunks asked again with fresh URLs');
      unstall('stalldata01');
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      await waitT(() => { const v = document.getElementById('ytc-video'); return v && v.currentTime >= 10 && !document.getElementById('ytc-player').classList.contains('stuck'); }, null, 6000);
      const ok = await incident(inc0, 'player-recovered');
      assert.ok(ok, 'player-recovered incident');
      assert.match(ok.reason, /recovered after .* \(1 retry with fresh URLs, MediaSource rebuilt\)$/);
      assert.ok(ok.player.notes.some(n => n.includes('data but no picture: a fresh MediaSource')), ok.player.notes.join('\n'));
      assert.deepEqual(problems(inc0).map(x => x.split(':')[0]), ['player-stuck', 'player-recovered']);
    } finally { unstall('stalldata01'); await page.setViewportSize({ width: 1400, height: 900 }); }
  });

  await step('own player: the stuck status\' Try again re-mounts at once with a fresh VISIONOS call', async () => {
    STALL.hold.set('stalltry001', 'all');
    try {
      await page.goto('https://www.youtube.com/watch?v=stalltry001', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-player.stuck', { timeout: 9000 });
      const n0 = visCalls('stalltry001').length, docs = log.docs.length;
      await page.locator('#ytc-pstuck .pfbt', { hasText: 'Try again' }).click();
      unstall('stalltry001');
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
      assert.equal(visCalls('stalltry001').length, n0 + 1, 'a fresh VISIONOS call');
      assert.equal(log.docs.length, docs, 'same page');
      assert.equal(await page.locator('#ytc-player.stuck').count(), 0);
    } finally { unstall('stalltry001'); }
  });

  await step('own player: a prefetched (cached) player response whose URLs hang → fresh URLs after 0.8 s, same mirror', async () => {
    await page.goto('https://www.youtube.com/results?search_query=cache+hang', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.evaluate(() => { const a = document.querySelector('#ytc-list a.t[href^="/watch"]'); a.setAttribute('href', '/watch?v=gvhangcach1'); });
    await page.hover('#ytc-list a.t[href="/watch?v=gvhangcach1"]');
    await page.waitForTimeout(700); // hover prefetch (VISIONOS cached)
    // The prefetch warms the video server's connection before the click (slow DNS/handshake, live 2 Nov 2026).
    assert.ok(await page.evaluate(() => !!document.querySelector('link[rel=preconnect][href="https://rr1---sn-fixture.googlevideo.com"][crossorigin]')), 'googlevideo preconnected on hover');
    const tc = Date.now();
    await page.click('#ytc-list a.t[href="/watch?v=gvhangcach1"]');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 12000);
    assert.ok(!page.url().includes('yt-native'), 'stays in our player');
    const g = log.gv.filter(x => x.v === 'gvhangcach1');
    assert.deepEqual([...new Set(g.map(x => x.gen))], [1, 2], 'cached URLs, then fresh ones');
    assert.ok(g.every(x => x.host === 'rr1---sn-fixture.googlevideo.com'), 'no mirror switch for a stale cache');
    // From the click: the hover media prefetch may already have asked the cached URLs before it.
    const t1 = Math.max(tc, Math.min(...g.filter(x => x.gen === 1).map(x => x.t))), t2 = Math.min(...g.filter(x => x.gen === 2).map(x => x.t));
    assert.ok(t2 - t1 < 1600, `fresh URLs soon after the cached ones hung: ${t2 - t1} ms`);
  });

  await step('own player: a stuck googlevideo node → the next mirror from the URL (like YouTube), then plays', async () => {
    await page.goto('https://www.youtube.com/watch?v=gvnodehang1', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 15000);
    assert.ok(!page.url().includes('yt-native'), 'stays in our player');
    const hosts = [...new Set(log.gv.filter(g => g.v === 'gvnodehang1').map(g => g.host))];
    assert.ok(hosts.includes('rr1---sn-fixture2.googlevideo.com'), 'asked the mirror: ' + hosts);
  });

  await step('own player: a slow googlevideo node (first answers 4 s late) → the same ranges on the mirror after 0.7 s → plays at once, no refresh', async () => {
    await page.goto('https://www.youtube.com/watch?v=gvslowfirst', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
    const ff = await page.evaluate(() => +document.documentElement.dataset.ytcFirstFrame);
    const g = log.gv.filter(x => x.v === 'gvslowfirst');
    assert.ok(!page.url().includes('yt-native'), 'stays in our player');
    assert.ok(g.some(x => x.host === 'rr1---sn-fixture2.googlevideo.com'), 'asked the mirror: ' + [...new Set(g.map(x => x.host))]);
    assert.deepEqual([...new Set(g.map(x => x.gen))], [1], 'no player refresh needed');
    assert.ok(ff < 2500, `first frame ${ff.toFixed(0)} ms after navigation (the slow node alone: > 4000)`);
    // Later requests go to the host that answered.
    await page.waitForTimeout(1500);
    const later = log.gv.filter(x => x.v === 'gvslowfirst' && x.t > Date.now() - 1200);
    assert.ok(later.every(x => x.host === 'rr1---sn-fixture2.googlevideo.com'), 'later requests on the mirror: ' + later.map(x => x.host));
  });

  await step('own player: a hung VISIONOS request → asked again after 0.4 s → plays (no 8 s timeout)', async () => {
    const n0 = visCalls('vishang0001').length;
    await page.goto('https://www.youtube.com/watch?v=vishang0001', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 6000);
    const ff = await page.evaluate(() => +document.documentElement.dataset.ytcFirstFrame);
    const calls = visCalls('vishang0001').slice(n0);
    assert.equal(calls.length, 2, 'two VISIONOS requests');
    assert.ok(calls[1].t - calls[0].t >= 350 && calls[1].t - calls[0].t < 1000, `the second after ~0.4 s: ${calls[1].t - calls[0].t} ms`);
    assert.ok(ff < 2000, `first frame ${ff.toFixed(0)} ms after navigation`);
    assert.equal(await page.locator('#ytc-player .pfail').count(), 0, 'no failure panel');
  });

  await step('own player: resting on a video link prefetches its first media (index + first segment); the click uses it, with no second request', async () => {
    await page.goto('https://www.youtube.com/results?search_query=media+prefetch', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.evaluate(() => { const a = document.querySelector('#ytc-list a.t[href^="/watch"]'); a.setAttribute('href', '/watch?v=mediapre001'); });
    await page.hover('#ytc-list a.t[href="/watch?v=mediapre001"]');
    for (let i = 0; i < 40 && log.gv.filter(g => g.v === 'mediapre001').length < 2; i++) await page.waitForTimeout(50);
    const pre = log.gv.filter(g => g.v === 'mediapre001');
    assert.equal(pre.length, 2, 'video and audio first requests while hovering: ' + JSON.stringify(pre));
    assert.ok(pre.every(g => g.a === 0 && g.b > 0), 'index + first segment ranges');
    await page.waitForTimeout(300);
    await page.click('#ytc-list a.t[href="/watch?v=mediapre001"]');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    const after = log.gv.filter(g => g.v === 'mediapre001').slice(2);
    assert.ok(!after.some(g => g.a === 0), 'the prefetched first requests were used, not asked again: ' + JSON.stringify(after));
  });

  await step('own player: a media prefetch still loading is cancelled when the pointer leaves the link (no stray requests)', async () => {
    STALL.hold.set('mediapre002', 'all');
    const aborted = [];
    const onFail = r => { if (r.url().includes('v=mediapre002')) aborted.push(r.failure()?.errorText || ''); };
    page.on('requestfailed', onFail);
    try {
      await page.goto('https://www.youtube.com/results?search_query=media+prefetch+leave', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c');
      await page.evaluate(() => { const a = document.querySelector('#ytc-list a.t[href^="/watch"]'); a.setAttribute('href', '/watch?v=mediapre002'); });
      await page.hover('#ytc-list a.t[href="/watch?v=mediapre002"]');
      for (let i = 0; i < 40 && log.gv.filter(g => g.v === 'mediapre002').length < 2; i++) await page.waitForTimeout(50);
      assert.equal(log.gv.filter(g => g.v === 'mediapre002').length, 2, 'prefetch started');
      await page.hover('#ytc-top');
      for (let i = 0; i < 40 && aborted.length < 2; i++) await page.waitForTimeout(50);
      assert.equal(aborted.length, 2, 'both requests cancelled: ' + aborted);
      await page.waitForTimeout(800);
      assert.equal(log.gv.filter(g => g.v === 'mediapre002').length, 2, 'nothing asked again');
    } finally { page.off('requestfailed', onFail); STALL.parked.length = 0; STALL.hold.delete('mediapre002'); }
  });

  await step('own player: a dropped connection → "No connection — continuing when it is back", then it plays by itself', async () => {
    NET.down = true; NET.probes = 0;
    try {
      await page.goto('https://www.youtube.com/watch?v=netdown0001', { waitUntil: 'commit' });
      await page.waitForFunction(() => /No connection/.test(document.querySelector('#ytc-player .pfm')?.textContent || ''), null, { timeout: 30000 });
      assert.ok(!page.url().includes('yt-native'), 'stays on our page');
      assert.ok(NET.probes >= 1, 'the network was probed');
      NET.down = false;
      await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 15000);
      assert.equal(await page.locator('#ytc-player .pfail').count(), 0, 'the panel is gone: it continued by itself');
    } finally { NET.down = false; }
  });

  await step('own player: a momentary 503 on the starting format is asked again (no fallback)', async () => {
    await page.goto('https://www.youtube.com/watch?v=flaky503vid', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
    assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    assert.equal((await vstate()).h, 360, 'kept the intended format');
    assert.equal(log.gv.filter(g => g.v === 'flaky503vid' && g.itag === '243' && g.a === 0).length, 2, 'index asked twice');
  });

  await step('own player: a rung whose chunks keep failing steps down instead of falling back', async () => {
    await page.goto('https://www.youtube.com/watch?v=stepdown001', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
    await waitT(() => document.getElementById('ytc-video')?.videoHeight === 240, null, 15000);
    assert.ok(!page.url().includes('yt-native'), 'no native fallback');
  });

  await step('own live: plays a live stream (HLS timeline, fMP4 segments by sq) near the live edge', async () => {
    const before = incidents.length, gv0 = log.gv.length, lv0 = log.live.length;
    LIVE.streams.delete('livestream1');
    await page.goto('https://www.youtube.com/watch?v=livestream1', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 10000);
    await waitT(() => { const v = document.getElementById('ytc-video'); return v && !v.paused && v.readyState >= 3; }, null, 6000);
    const st = LIVE.streams.get('livestream1'), head = liveHead(st);
    const segs = log.gv.slice(gv0).filter(g => g.live);
    const first = segs[0].sq;
    // Start ≥ 2 target durations (4 s → 8 s) behind the end of the newest segment: 1003 of 1000…1005.
    assert.ok(first >= LIVE.base + 3 && first <= LIVE.base + 4, `started at sq ${first} (head ${head})`);
    assert.deepEqual([...new Set(segs.filter(g => g.sq === first).map(g => g.itag))].sort(), ['134', '140'], 'video + audio of the same sq');
    const lv = log.live.slice(lv0);
    assert.equal(lv.filter(x => x.p.includes('/hls_variant/')).length, 1, 'master fetched once');
    assert.ok(lv.some(x => x.p.includes('/itag/230/')), 'follows the 360p variant playlist');
    const s = await vstate();
    assert.ok(Math.abs(s.t - first * LIVE.fx.SEG) < LIVE.fx.SEG + 1, `media time ${s.t} ≈ sq ${first} × ${LIVE.fx.SEG}`);
    assert.equal(s.h, 360);
    const cls = await page.getAttribute('#ytc-player', 'class');
    assert.match(cls, /\blive\b/); assert.match(cls, /\bedge\b/);
    assert.ok(await page.locator('.plive').isVisible(), 'LIVE button');
    if (process.env.YTC_SHOTS) { await page.mouse.move(700, 400); await page.waitForTimeout(100); const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'player-live.png'), clip: { x: b.x, y: b.y + b.height - 70, width: b.width, height: 70 } }); }
    assert.equal(await page.textContent('#ytc-time'), '');
    assert.ok(!page.url().includes('yt-native'));
    assert.deepEqual(problems(before), []);
  });

  await step('own live: the playlist refreshes every target duration and new segments play (latency stays small)', async () => {
    const lv0 = log.live.length, gv0 = log.gv.length, t0 = (await vstate()).t;
    await page.waitForTimeout(7000);
    const polls = log.live.slice(lv0).filter(x => x.p.includes('/hls_playlist/'));
    assert.ok(polls.length >= 1 && polls.length <= 4, `${polls.length} playlist refreshes in 7 s (target 4 s)`);
    const st = LIVE.streams.get('livestream1'), head = liveHead(st);
    const fetched = log.gv.slice(gv0).filter(g => g.live && g.itag === '134').map(g => g.sq);
    assert.ok(fetched.length >= 1 && Math.max(...fetched) >= head - 1, `new segments fetched: ${fetched} (head ${head})`);
    const s = await vstate();
    assert.ok(s.t - t0 > 5, `played on: ${t0} → ${s.t}`);
    const edgeT = (head + 1) * LIVE.fx.SEG;
    assert.ok(edgeT - s.t < 15, `${(edgeT - s.t).toFixed(1)} s behind the newest segment`);
    // Headless Chromium decodes in software: under machine load it drops a frame or two.
    assert.ok(s.dropped <= Math.max(2, s.frames * 0.02), `dropped ${s.dropped} of ${s.frames} frames`);
    await page.keyboard.press('Backquote');
    const dbg = await page.locator('#ytc-debug').textContent();
    assert.match(dbg, /live · window sq \d+–\d+ .* at the edge · latency \d+\.\d s/);
    assert.match(dbg, /live latency \d+\.\d s behind the broadcast/);
    await page.keyboard.press('Backquote');
  });

  await step('own live: DVR seek back leaves the edge (−m:ss), LIVE returns to it', async () => {
    const before = (await vstate()).t;
    await page.keyboard.press('j');
    await waitT(() => !/\bedge\b/.test(document.getElementById('ytc-player').className), null, 3000);
    await waitT(() => /^−0:\d\d$/.test(document.getElementById('ytc-time').textContent), null, 3000);
    await waitT(b => { const v = document.getElementById('ytc-video'); return v.currentTime < b - 5 && v.readyState >= 3 && !v.paused; }, before, 8000);
    await page.locator('.plive').click();
    await waitT(() => /\bedge\b/.test(document.getElementById('ytc-player').className), null, 3000);
    await waitT(b => document.getElementById('ytc-video').currentTime > b, before, 5000);
  });

  await step('own live: a stream that ends plays to its last segment, then ended (Out::Ended)', async () => {
    const before = incidents.length;
    LIVE.streams.delete('liveended01');
    await page.goto('https://www.youtube.com/watch?v=liveended01', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 20000);
    await page.evaluate(() => { document.getElementById('ytc-video').playbackRate = 2; });
    await waitT(() => document.getElementById('ytc-video').ended && document.documentElement.dataset.ytcEnded, null, 25000);
    const s = await vstate();
    assert.ok(Math.abs(s.t - (LIVE.base + 7) * LIVE.fx.SEG) < 0.5, `ended at ${s.t}`);
    assert.deepEqual(problems(before), []);
    assert.ok(!page.url().includes('yt-native'));
  });

  await step('own live: upcoming streams/premieres show YouTube\'s words in the panel (the page stays), without an incident', async () => {
    const before = incidents.length;
    await page.goto('https://www.youtube.com/watch?v=liveupcomin', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-player.failed .pfail', { timeout: 8000 });
    await page.waitForTimeout(500);
    assert.deepEqual(await panel(), { url: '?v=liveupcomin', m: 'Premieres in 2 hours', r: 'upcoming live stream or premiere: Premieres in 2 hours' });
    assert.deepEqual(problems(before), []);
  });

  await step('own player: a better quality chosen while paused redraws the paused frame (no resume needed)', async () => {
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button[data-arg="q:144"]').click();
    await waitT(() => document.getElementById('ytc-video').videoHeight === 144, null, 9000);
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    await page.waitForTimeout(300);
    const t0 = await page.evaluate(() => document.getElementById('ytc-video').currentTime);
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button[data-arg="q:0"]').click();
    await waitT(() => document.getElementById('ytc-video').videoHeight === 360, null, 9000);
    const st = await vstate();
    assert.ok(st.paused, 'still paused');
    assert.ok(Math.abs(st.t - t0) < 0.3, `same position (${t0} → ${st.t})`);
  });

  await step('own player: an unreachable starting format is replaced before playback', async () => {
    await page.goto('https://www.youtube.com/watch?v=deadstart01', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 9000);
    await page.waitForTimeout(800);
    assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    const st = await vstate();
    assert.ok(!st.paused && st.h === 240, JSON.stringify(st));
  });

  await step('own player: an unreachable quality switch keeps playing the current one', async () => {
    // Remember 144p first; switching up to 360p then meets a format googlevideo
    // will not serve, so playback must stay on 144p.
    await page.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button[data-arg="q:144"]').click();
    await page.waitForTimeout(300);
    await page.goto('https://www.youtube.com/watch?v=deadupgrade', { waitUntil: 'commit' });
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await page.click('#ytc-qual');
    await page.waitForSelector('#ytc-player.menu #ytc-menu button');
    await page.locator('#ytc-menu button[data-arg="q:360"]').click();
    for (let i = 0; i < 40 && !log.gv.some(g => g.v === 'deadupgrade' && g.itag === '243'); i++) await page.waitForTimeout(100);
    await page.waitForTimeout(1500);
    assert.ok(log.gv.some(g => g.v === 'deadupgrade' && g.itag === '243'), 'the switch was attempted');
    assert.ok(!page.url().includes('yt-native'), 'no native fallback');
    const st = await vstate();
    assert.ok(!st.paused && st.h === 144, JSON.stringify(st));
    await page.click('#ytc-qual');
    await page.locator('#ytc-menu button[data-arg="q:0"]').click();
  });

  await step('layout: six-link sidebar (no Shorts) while browsing, none on watch pages; results render without layout shift', async () => {
    await page.goto('https://www.youtube.com/results?search_query=layout', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForTimeout(150);
    const cls = await page.evaluate(() => new Promise(r => { let s = 0; new PerformanceObserver(l => { for (const e of l.getEntries()) if (!e.hadRecentInput) s += e.value; }).observe({ type: 'layout-shift', buffered: true }); setTimeout(() => r(s), 100); }));
    console.log(`     cumulative layout shift (search, cold): ${cls.toFixed(4)}`);
    assert.ok(cls < 0.05, 'CLS ' + cls);
    assert.deepEqual(await page.locator('#ytc-side a').allTextContents(), ['Home', 'History', 'Watch later', 'Subscriptions', 'Playlists', 'Liked videos']);
    assert.ok(await page.locator('#ytc-side').isVisible());
    const search = await page.locator('#ytc-search').boundingBox();
    assert.ok(search.width <= 721 && Math.abs(search.x + search.width / 2 - 700) < 2, 'search centered, ≤720px: ' + JSON.stringify(search));
    const first = await page.locator('#ytc-list .c').first().boundingBox();
    assert.ok(first.x >= 224, 'content clears the sidebar');
    await page.click('#ytc-list a.t[href^="/watch"] >> nth=0');
    await page.waitForSelector('#ytc-info h1.wt');
    assert.equal(await page.getAttribute('#ytc', 'data-nav'), 'watch');
    assert.ok(!(await page.locator('#ytc-side').isVisible()), 'no sidebar on watch');
    await morphDone();
    assert.ok((await page.locator('#ytc-player').boundingBox()).x < 40, 'player uses the released width');
    for (const id of ['ytc-sub', 'ytc-acts', 'ytc-comments']) assert.equal(await page.locator('#' + id).count(), 1, id);
    await morphDone();
    const pl = await page.locator('#ytc-player').boundingBox();
    assert.ok(pl.height >= 0.55 * 900 && Math.abs(pl.width / pl.height - 16 / 9) < 0.02, 'the player dominates: ' + JSON.stringify(pl));
    // The description is fully open at its natural height: no clamp and no Show more.
    assert.deepEqual(await page.evaluate(() => { const d = document.getElementById('ytc-desc'), cs = getComputedStyle(d); return [d.classList.contains('clamp'), !!d.querySelector('button.lnk'), cs.maxHeight, cs.overflow, d.scrollHeight <= d.clientHeight + 1]; }), [false, false, 'none', 'visible', true], 'description fully open');
  });

  await step('keyboard-first: arrows walk the cards (rows by layout), Enter opens, g+letter jumps, ? shows the sheet', async () => {
    await page.goto('https://www.youtube.com/results?search_query=keys', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const idx = () => page.evaluate(() => [...document.querySelectorAll('#ytc-list a.t')].indexOf(document.activeElement));
    await page.keyboard.press('ArrowRight');
    assert.equal(await idx(), 0, 'first card in view');
    await page.keyboard.press('ArrowRight');
    assert.equal(await idx(), 1);
    const cols = await page.evaluate(() => getComputedStyle(document.getElementById('ytc-list')).gridTemplateColumns.split(' ').length);
    await page.keyboard.press('ArrowDown');
    assert.equal(await idx(), 1 + cols, `one row down (${cols} columns)`);
    await page.keyboard.press('ArrowUp');
    assert.equal(await idx(), 1);
    const href = await page.evaluate(() => document.activeElement.getAttribute('href'));
    const docs = log.docs.length;
    await page.keyboard.press('Enter');
    await page.waitForFunction(h => location.pathname + location.search === h, href);
    assert.equal(log.docs.length, docs, 'SPA');
    await page.waitForSelector('#ytc-info h1.wt');
    await page.keyboard.press('Shift+?');
    await page.waitForSelector('#ytc-keys.on');
    assert.ok((await page.locator('#ytc-keys').textContent()).includes('Watch later'));
    assert.ok(!(await page.locator('#ytc-keys').textContent()).includes('0–9'), 'no 0–9 row (blocked keys)');
    await page.keyboard.press('Escape');
    await page.waitForSelector('#ytc-keys:not(.on)', { state: 'attached' });
    await page.keyboard.press('g');
    await page.keyboard.press('h');
    await page.waitForFunction(() => location.pathname === '/' && document.getElementById('ytc').dataset.nav === 'home');
  });

  // ---- motion (the motion law): the watch-open morph and its exceptions
  // Animations on the watch page's player/info/related/comments (WAAPI and CSS), as [target, kind, first keyframe].
  // (Comments appended after the first frame settle in on any navigation: `withComments` only for restores.)
  const mainAnims = (withComments = false) => page.evaluate(wc => document.getAnimations().filter(a => {
    const t = a.effect && a.effect.target; return t && (t.closest('#ytc-player') === t || ['ytc-info', 'ytc-desc', 'ytc-list', 'ytc-pl'].includes(t.id) || (wc && t.classList.contains('ycm')) || t.classList.contains('upc'));
  }).map(a => [a.effect.target.id || a.effect.target.className, a.id || a.constructor.name, JSON.stringify(a.effect.getKeyframes()[0] || {})]), withComments);
  // The thumbnail of card `i` on a results page, and a pointer click on it (real mouse events).
  const thumbAt = i => page.evaluate(i => { const r = document.querySelectorAll('#ytc-list .c[data-v] a.th')[i].getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; }, i);
  await step('motion: a pointer click morphs the player from the clicked thumbnail (spring, transform only); final and interactive on frame one', async () => {
    await page.goto('https://www.youtube.com/results?search_query=morph', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.mouse.move(2, 2);
    const th = await thumbAt(1);
    await page.mouse.click(th.x + th.w / 2, th.y + th.h / 2);
    // Same task as the swap: the page is complete when the animation exists.
    const m = await page.evaluate(async () => {
      const p = document.getElementById('ytc-player'), a = p.getAnimations().find(x => x.id === 'flip');
      if (!a) return null;
      const k = a.effect.getKeyframes(), t = a.effect.getTiming(), now = p.getBoundingClientRect();
      const hit = document.elementFromPoint(now.left + now.width / 2, now.top + now.height / 2);
      const out = { from: k[0].transform, to: k[k.length - 1].transform, origin: k[0].transformOrigin, easing: t.easing, ms: t.duration, props: Object.keys(k[0]).filter(x => !['offset', 'easing', 'composite', 'computedOffset'].includes(x)),
        state: a.playState, title: !!document.querySelector('#ytc-info h1.wt'), hitPlayer: !!(hit && hit.closest('#ytc-player')), flipClass: p.dataset.flip === '1', z: getComputedStyle(p).zIndex };
      a.finish();
      const r = p.getBoundingClientRect(); out.final = { x: r.left, y: r.top, w: r.width, h: r.height };
      await a.finished; await new Promise(res => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(res))));
      out.flipAfter = 'flip' in p.dataset;
      return out;
    });
    assert.ok(m, 'a flip animation on #ytc-player');
    assert.equal(m.state, 'running');
    assert.deepEqual(m.props.sort(), ['transform', 'transformOrigin'], 'compositor-only: ' + m.props);
    assert.equal(m.to, 'none');
    assert.ok(/^linear\(0[ ,]/.test(m.easing) && m.easing.split(',').length === 25 && m.ms === 458, 'soft spring from motion.rs: ' + m.easing.slice(0, 30) + ' ' + m.ms);
    const [, tx, ty, sx, sy] = /translate\(([-\d.e]+)px, ?([-\d.e]+)px\) scale\(([\d.e-]+), ?([\d.e-]+)\)/.exec(m.from).map(Number);
    const start = { x: m.final.x + tx, y: m.final.y + ty, w: m.final.w * sx, h: m.final.h * sy };
    for (const k of ['x', 'y', 'w', 'h']) assert.ok(Math.abs(start[k] - th[k]) < 1, `morph starts on the thumbnail (${k}): ${JSON.stringify(start)} vs ${JSON.stringify(th)}`);
    assert.ok(m.title && m.hitPlayer, 'page complete and the player takes clicks where it is painted on frame one');
    assert.ok(m.flipClass && m.z === '3' && !m.flipAfter, 'above its neighbours while moving, class removed at the end: ' + JSON.stringify([m.flipClass, m.z, m.flipAfter]));
    assert.ok(m.final.x < 40 && m.final.w > 900, 'final player box: ' + JSON.stringify(m.final));
    // The title skeleton has the final geometry; the description skeleton has the views/date-only panel's, so
    // when next lands nothing moves up and resource lines only extend the panel downwards.
    await page.waitForSelector('#ytc-desc:not(.sk)');
    const g = await page.evaluate(() => {
      const info = document.getElementById('ytc-info'), el = (tag, cls, kid) => { const e = document.createElement(tag); e.className = cls; if (kid) e.append(kid); return e; };
      const add = e => { info.append(e); const r = e.offsetHeight; e.remove(); return r; };
      return { title: document.querySelector('#ytc-info h1.wt').offsetHeight, titleSk: add(el('h1', 'wt', el('i', 'sk l w60'))), desc: document.getElementById('ytc-desc').offsetHeight, descSk: add(el('div', 'desc sk')), metaOnly: add(el('section', 'desc', el('div', 'ws', el('span', '', '1 view')))) };
    });
    assert.ok(Math.abs(g.title - g.titleSk) < 0.5 && Math.abs(g.metaOnly - g.descSk) < 0.5 && g.desc >= g.descSk, 'skeleton geometry: ' + JSON.stringify(g));
    // A click on the player right away reaches it (no input lock during motion).
    await page.mouse.click(m.final.x + m.final.w / 2, m.final.y + m.final.h / 3);
    assert.equal(errors.length, 0, errors.join('\n'));
    // Theater: a pointer click morphs the player into its new box (--spring, 351 ms); the t key is instant.
    await page.mouse.move(m.final.x + m.final.w / 2, m.final.y + m.final.h / 2);
    await page.locator('#ytc-player button[data-act="p-theater"]').click();
    const th1 = await page.evaluate(() => { const a = document.getElementById('ytc-player').getAnimations().find(x => x.id === 'flip'); const r = a && a.effect.getTiming().duration; if (a) a.finish(); return [document.getElementById('ytc-watch').className, r]; });
    assert.deepEqual(th1, ['watch theater', 351], 'theater by pointer morphs');
    await page.mouse.move(2, 2);
    await page.keyboard.press('t');
    const th2 = await page.evaluate(() => [document.getElementById('ytc-watch').className, document.getElementById('ytc-player').getAnimations().some(x => x.id === 'flip')]);
    assert.deepEqual(th2, ['watch', false], 't is instant');
    if (process.env.YTC_SHOTS) {
      // Frames of the morph (paused at fixed times) for review.
      await page.goto('https://www.youtube.com/results?search_query=morphshots', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.waitForTimeout(800);
      await page.mouse.move(2, 2);
      const t2 = await thumbAt(1);
      await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'morph-0-before.png') });
      await page.mouse.click(t2.x + t2.w / 2, t2.y + t2.h / 2);
      for (const ms of [0, 40, 100, 200, 458]) {
        await page.evaluate(ms => { for (const a of document.getAnimations()) { a.pause(); a.currentTime = Math.min(ms, a.effect.getTiming().duration); } }, ms);
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `morph-${ms}.png`) });
      }
      await page.evaluate(() => { for (const a of document.getAnimations()) a.finish(); });
    }
  });

  await step('motion: keyboard Enter, back/forward and restored pages are instant; reduced motion fades the player instead', async () => {
    // Keyboard: Enter on a focused card opens the watch page without any animation.
    await page.goto('https://www.youtube.com/results?search_query=morphkeys', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.mouse.move(2, 2);
    await page.evaluate(() => document.querySelectorAll('#ytc-list a.t[href^="/watch"]')[2].focus());
    await page.waitForTimeout(400); // focus is intent: its next prefetch lands, so Enter paints the whole page at once
    await page.evaluate(() => { window.__still = []; new MutationObserver(() => window.__still.push(document.getElementById('ytc').dataset.still)).observe(document.getElementById('ytc'), { attributes: true, attributeFilter: ['data-still'] }); });
    await page.keyboard.press('Enter');
    await page.waitForSelector('#ytc-info h1.wt');
    assert.deepEqual(await mainAnims(), [], 'no motion after keyboard Enter');
    await page.waitForFunction(() => window.__still.length >= 2);
    assert.deepEqual(await page.evaluate(() => window.__still), ['1', '0'], 'CSS entrances off for the first frame only');
    // Comments appended later (pointer-independent) do settle in.
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    await page.waitForSelector('#ytc-cm-list > .ycm');
    // Back to results, forward to the watch page (restored from memory): instant.
    await page.evaluate(() => history.back());
    await page.waitForFunction(() => location.pathname === '/results' && document.querySelector('#ytc-list .c[data-v]'));
    assert.deepEqual(await mainAnims(true), [], 'no motion on back');
    await page.evaluate(() => history.forward());
    await page.waitForFunction(() => location.pathname === '/watch' && document.querySelector('#ytc-cm-list > .ycm'));
    assert.deepEqual(await mainAnims(true), [], 'no motion on forward (restored page with its comments)');
    // Reduced motion: the pointer click fades the player in, no transform.
    await page.emulateMedia({ reducedMotion: 'reduce' });
    try {
      await page.goto('https://www.youtube.com/results?search_query=morphrm', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      const th = await thumbAt(0);
      await page.mouse.click(th.x + th.w / 2, th.y + th.h / 2);
      const k = await page.evaluate(() => document.getElementById('ytc-player').getAnimations().map(a => a.effect.getKeyframes().map(f => Object.keys(f).filter(x => !['offset', 'easing', 'composite', 'computedOffset'].includes(x)).join() + ':' + (f.opacity ?? '')).join(' ')));
      assert.deepEqual(k, ['opacity:0 opacity:1'], 'reduced motion: one opacity fade, no transform');
    } finally { await page.emulateMedia({ reducedMotion: null }); }
  });

  await step('motion: the morph costs the click no frame (pointer click vs keyboard-style click, same cached page, median of 11)', async () => {
    // click event → the next frame begins (rAF) → that frame has painted (rAF + task), median of 7 each.
    const once = async kind => {
      await page.goto('https://www.youtube.com/results?search_query=morphtime', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.mouse.move(2, 2);
      await page.waitForTimeout(150);
      // (The bubble listener on window runs after the client's document listener: the click task's own work.)
      await page.evaluate(() => { window.__t = null; let t0 = 0, task = 0; addEventListener('click', () => { task = performance.now() - t0; }, { once: true }); addEventListener('click', () => { t0 = performance.now(); requestAnimationFrame(() => { const f = performance.now(); setTimeout(() => { window.__t = { task, frame: f - t0, painted: performance.now() - t0 }; }); }); }, { capture: true, once: true }); });
      if (kind === 'pointer') { const th = await thumbAt(1); await page.mouse.click(th.x + th.w / 2, th.y + th.h / 2); }
      else await page.evaluate(() => document.querySelectorAll('#ytc-list .c[data-v] a.th')[1].dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 0 })));
      await page.waitForFunction(() => window.__t);
      const r = await page.evaluate(() => ({ ...window.__t, morph: document.getElementById('ytc-player').getAnimations().some(a => a.id === 'flip') }));
      if (!warm) assert.equal(r.morph, kind === 'pointer', kind + ' ' + JSON.stringify(r));
      return r;
    };
    let warm = true;
    await once('pointer'); // warm: next + VISIONOS cached for this card
    warm = false;
    const rows = { pointer: [], key: [] };
    for (let i = 0; i < 11; i++) { rows.pointer.push(await once('pointer')); rows.key.push(await once('key')); }
    const md = (k, f) => median(rows[k].map(r => r[f])).toFixed(1);
    console.log(`     median of 11: click task (swap + morph setup) ${md('pointer', 'task')} ms vs ${md('key', 'task')} ms without; click → next frame starts ${md('pointer', 'frame')} vs ${md('key', 'frame')} ms; → that frame painted ${md('pointer', 'painted')} vs ${md('key', 'painted')} ms`);
  });

  await step('search: YouTube\'s recommendation shelves (People also watched, For you, People also search for) are dropped; a channel\'s linked shelf and playlist results stay; same in continuations', async () => {
    const mark = log.api.length;
    await page.goto('https://www.youtube.com/results?search_query=shelves', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const has = id => page.locator(`#ytc-list .c[data-v="${id}"]`).count();
    for (const id of [...SHELF_IDS.rec.slice(0, 2), 'AH30avEEC9A', '1AGWEnrPeQY']) assert.equal(await has(id), 0, `recommended ${id} not shown`);
    for (const id of SHELF_IDS.keep) assert.equal(await has(id), 1, `channel shelf video ${id} kept`);
    const titles = await page.locator('#ytc-list .c .t').allTextContents();
    assert.ok(titles.includes('Roblox Animation Tutorials'), 'playlist result kept');
    assert.ok(!(await page.locator('#ytc-list').textContent()).includes('People also search for'), 'no refinement row');
    // Order: the kept shelf sits where YouTube put it, between real results.
    const ids = await page.$$eval('#ytc-list .c[data-v]', cs => cs.map(c => c.dataset.v));
    assert.ok(ids.indexOf(SHELF_IDS.keep[0]) >= 0 && ids.indexOf(SHELF_IDS.keep[1]) === ids.indexOf(SHELF_IDS.keep[0]) + 1 && ids.indexOf('TlgW8jhLsc0') === ids.indexOf(SHELF_IDS.keep[1]) + 1, ids.join(','));
    await page.evaluate(() => scrollTo(0, document.body.scrollHeight));
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length > 20, null, { timeout: 5000 });
    assert.ok(log.api.slice(mark).some(a => a.ep === 'search' && a.body.continuation === SHELF_CONT), 'continuation asked');
    assert.equal(await has(SHELF_IDS.rec[2]), 0, 'People also watched in the continuation dropped');
    // The debug overlay counts them.
    await page.keyboard.press('Backquote');
    await page.waitForSelector('#ytc-debug.on');
    const dbg = await page.locator('#ytc-debug').textContent();
    assert.ok(/recommendation shelves [4-9]/.test(dbg), dbg.slice(0, 400));
    await page.keyboard.press('Backquote');
    await shot('search-no-rec-shelves');
    await page.evaluate(() => scrollTo(0, 0));
  });

  await step('debug overlay toggles with ` and shows parse counts', async () => {
    await page.goto('https://www.youtube.com/results?search_query=roblox', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.keyboard.press('Backquote');
    await page.waitForSelector('#ytc-debug.on');
    const txt = await page.locator('#ytc-debug').textContent();
    assert.match(txt, /shorts hidden [1-9]/);
    assert.match(txt, /client WEB 2\.\d{8}/);
    await page.keyboard.press('Backquote');
    await page.waitForSelector('#ytc-debug:not(.on)', { state: 'attached' });
  });

  await step('config: sw.js_data refresh stored in chrome.storage (small config only)', async () => {
    for (let i = 0; i < 40 && !log.api.some(a => a.ep === 'sw.js_data'); i++) await page.waitForTimeout(100);
    assert.ok(log.api.some(a => a.ep === 'sw.js_data'), 'sw.js_data fetched');
    await page.waitForTimeout(200);
    await page.goto('https://www.youtube.com/results?search_query=config', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const s = log.api.filter(a => a.ep === 'search').pop();
    assert.equal(s.headers['x-youtube-client-version'], '2.20261029.11.00', 'version from sw.js_data');
    assert.ok(s.headers['x-goog-visitor-id']?.startsWith('Cgt'), 'visitor id header');
    assert.equal(s.body.context.client.visitorData, s.headers['x-goog-visitor-id']);
  });

  await step('native mode (?yt-native=1, the panel\'s Open on YouTube) loads YouTube with its bundles; no escape link in the header', async () => {
    assert.equal(await page.locator('#ytc-native').count(), 0, 'no "Open on YouTube" link');
    await page.goto('https://www.youtube.com/results?search_query=roblox&yt-native=1', { waitUntil: 'commit' });
    await page.waitForFunction(() => window.__nativeBundle === 1, null, { timeout: 5000 });
    assert.equal(await page.evaluate(() => window.__ytAppRan), true);
    assert.equal(await page.locator('#ytc-top').count(), 0);
    assert.ok(log.assets.some(u => u.includes('/s/desktop/abc123/jsbin/')), 'native bundle allowed');
    const r = await page.evaluate(async () => { try { await fetch('/youtubei/v1/log_event?alt=json'); return 'ok'; } catch { return 'blocked'; } });
    assert.equal(r, 'blocked', 'telemetry still blocked in native mode');
  });

  await step('excluded native paths are left alone (/account)', async () => {
    await page.goto('https://www.youtube.com/account', { waitUntil: 'commit' });
    await page.waitForFunction(() => window.__nativeBundle === 1, null, { timeout: 5000 });
    assert.equal(await page.locator('#ytc-top').count(), 0);
  });

  await step('pages the client does not render open YouTube\'s own page directly', async () => {
    await page.goto('https://www.youtube.com/hashtag/roblox', { waitUntil: 'commit' });
    await page.waitForURL(u => u.search.includes('yt-native=1'), { timeout: 5000 });
    assert.equal(new URL(page.url()).pathname, '/hashtag/roblox');
  });

  // ---- Native-page cleanup (2 Nov 2026): YouTube's own pages keep the client's setup. The page is a
  // fixture of YouTube's DOM assembled from the old extension's verified test fixtures.
  const NATIVE_FX = 'https://www.youtube.com/watch?v=ntvfixture1&yt-native=1';
  const cfgBefore = await sw.evaluate(() => chrome.storage.local.get(['ytcConfigV1', 'shortsInFeeds']));
  const setStore = o => sw.evaluate(o => chrome.storage.local.set(o), o);
  const getCfg = () => sw.evaluate(() => chrome.storage.local.get('ytcConfigV1').then(r => r.ytcConfigV1 || {}));
  const editCfg = p => sw.evaluate(p => chrome.storage.local.get('ytcConfigV1').then(r => { const c = r.ytcConfigV1 || {}; c.p = { ...(c.p || {}), ...p }; return chrome.storage.local.set({ ytcConfigV1: c }); }), p);
  const openNative = async () => {
    await page.goto(NATIVE_FX, { waitUntil: 'commit' });
    await page.waitForSelector('html[data-ytc-native] #movie_player', { timeout: 8000 });
    await page.waitForTimeout(150);
  };
  // Which of these ids are rendered (checkVisibility: display/visibility of the element and its ancestors).
  const shown = ids => page.evaluate(ids => Object.fromEntries(ids.map(id => { const el = document.getElementById(id); return [id, !!el && el.checkVisibility()]; })), ids);
  const expectShown = async (want, ids) => {
    const got = await shown(ids);
    const wrong = ids.filter(id => got[id] !== want);
    assert.deepEqual(wrong, [], (want ? 'hidden but should show: ' : 'shown but should be hidden: ') + wrong.join(', '));
  };
  await step('native page cleanup: header, sidebar, Shorts, Playables and promotions as in the old extension (YouTube DOM fixture)', async () => {
    await setStore({ shortsInFeeds: false });
    await openNative();
    assert.equal(await page.evaluate(() => window.__ytAppRan), true, 'YouTube\'s own page runs');
    await expectShown(false, ['voice-search-button', 'country-code', 'create', 'logo-icon']);
    await expectShown(true, ['logo', 'search', 'notifications', 'account']);
    assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('logo'), '::after').content), '"YouTube"', 'plain YouTube wordmark');
    // Sidebar: the six browsing links; Liked videos out of the collapsed part.
    await expectShown(true, ['home', 'subscriptions-nav', 'history', 'playlists', 'watch-later', 'liked-videos', 'mini-home', 'mini-subscriptions']);
    await expectShown(false, ['shorts-nav', 'mini-shorts', 'your-channel', 'your-videos', 'downloads', 'podcasts', 'courses', 'expander-item', 'subscriptions', 'subscribed-channel', 'explore', 'more-youtube', 'report-history', 'footer']);
    // Shorts and Playables out of feeds; ordinary and channel cards stay.
    await expectShown(false, ['short-video', 'absolute-short', 'short-shelf', 'playable-card']);
    await expectShown(true, ['normal-video', 'channel-link']);
    // Promotions: banners hidden, the offer dialog closed by its own dismiss button (never the trial),
    // the account notice mealbar left alone.
    await expectShown(false, ['membership-section', 'premium-section']);
    await page.waitForFunction(() => window.__dismissed === 1, null, { timeout: 3000 });
    await page.waitForTimeout(1700);
    assert.deepEqual(await page.evaluate(() => [window.__dismissed, window.__trial, window.__notice]), [1, 0, 0]);
    await expectShown(true, ['notice', 'notice-action']);
    assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).backgroundColor), 'rgb(16, 19, 21)', 'client palette');
  });

  await step('native page cleanup: watch page (Join, Thanks, overflow, Auto-dubbed, Summary/Transcript/Video details, related chips) and player overlays', async () => {
    await expectShown(false, ['join-host', 'thanks-host', 'overflow', 'dubbed', 'super-title', 'summary', 'games', 'gaming', 'native-transcript', 'repeat-channel', 'video-details', 'related-filters']);
    await expectShown(true, ['like', 'dislike', 'share', 'save', 'subscribe', 'owner-renderer', 'full-transcript', 'attributed-description', 'quality-badge']);
    const player = await page.evaluate(() => {
      const hid = s => !document.querySelector(s).checkVisibility();
      return { overlays: ['.ytp-fullscreen-grid', '.ytp-more-videos-button', '.ytp-fullscreen-quick-actions', '.ytp-fullscreen-metadata', '.iv-branding', '.ytp-gradient-top', '.ytp-gradient-bottom', '#cinematics'].filter(s => !hid(s)),
        bg: getComputedStyle(document.querySelector('.html5-video-container')).backgroundImage + ' ' + getComputedStyle(document.getElementById('movie_player')).backgroundColor,
        progress: getComputedStyle(document.querySelector('.ytp-play-progress')).backgroundColor, controls: !hid('.ytp-play-button') && !hid('.ytp-subtitles-button') };
    });
    assert.deepEqual(player.overlays, [], 'fullscreen grid/quick actions/metadata, watermark, gradients and cinematics hidden');
    assert.match(player.bg, /none rgb\(0, 0, 0\)/, 'solid black around the picture');
    assert.equal(player.progress, 'rgb(183, 235, 202)', 'Scribe-mint progress');
    assert.ok(player.controls, 'playback controls stay');
  });

  await step('native page: captions in the client\'s style and place; drag, wheel and the size range write ytcConfigV1.p.cs', async () => {
    await editCfg({ cs: { v: 2, x: 0.1, y: 0.2, s: 1 }, cc: true });
    await openNative();
    const box = () => page.evaluate(() => {
      const f = document.querySelector('.ytp-caption-window-container').getBoundingClientRect(), c = document.getElementById('caption-window-1').getBoundingClientRect();
      const seg = getComputedStyle(document.querySelector('#caption-window-1 .ytp-caption-segment'));
      return { x: (c.left - f.left) / f.width, y: (c.top - f.top) / f.height, cx: c.left + Math.min(c.width / 2, 40), cy: c.top + c.height / 2, color: seg.color, bg: seg.backgroundColor, font: parseFloat(seg.fontSize), family: seg.fontFamily, notice: document.getElementById('caption-window-_notice').checkVisibility() };
    });
    let b = await box();
    assert.ok(Math.abs(b.x - 0.1) < 0.002 && Math.abs(b.y - 0.2) < 0.002, 'shared position: ' + JSON.stringify(b));
    assert.equal(b.color, 'rgb(0, 255, 255)');
    assert.equal(b.bg, 'rgba(0, 0, 0, 0)');
    assert.match(b.family, /^Arial/);
    // The client's size formula: clamp(16px, 5cqh, 72px) × .75 × s → 540 px player: 27 × .75 = 20.25 px.
    assert.ok(Math.abs(b.font - 20.25) < 0.5, 'size ' + b.font);
    assert.equal(b.notice, false, 'the "click gear for settings" notice is hidden');
    // Drag: YouTube never sees the press (no native drag, no pause), the new place is saved for the client.
    await page.mouse.move(b.cx, b.cy);
    await page.mouse.down();
    await page.mouse.move(b.cx + 48, b.cy + 27, { steps: 4 });
    await page.mouse.move(b.cx + 96, b.cy + 54, { steps: 4 });
    await page.mouse.up();
    b = await box();
    assert.ok(Math.abs(b.x - 0.2) < 0.003 && Math.abs(b.y - 0.3) < 0.003, 'moved: ' + JSON.stringify(b));
    assert.deepEqual(await page.evaluate(() => [window.__nativeCaptionDrag, window.__playerClicks]), [0, 0], 'no native drag or play toggle');
    for (let i = 0; i < 30 && Math.abs(((await getCfg()).p?.cs?.x ?? 0) - 0.2) > 0.003; i++) await page.waitForTimeout(100);
    let cs = (await getCfg()).p.cs;
    assert.ok(cs.v === 2 && Math.abs(cs.x - 0.2) < 0.003 && Math.abs(cs.y - 0.3) < 0.003 && cs.s === 1, JSON.stringify(cs));
    // Wheel over the cue: two size steps up, the page does not scroll; saved once the wheel rests.
    const y0 = await page.evaluate(() => scrollY);
    await page.mouse.move(b.cx, b.cy);
    await page.mouse.wheel(0, -100);
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => scrollY), y0, 'page did not scroll');
    for (let i = 0; i < 30 && (await getCfg()).p.cs.s !== 1.1; i++) await page.waitForTimeout(100);
    assert.equal((await getCfg()).p.cs.s, 1.1);
    // The size range beside the cue shows on hover, mirrors the size and sets it.
    await page.mouse.move(b.cx, b.cy);
    await page.waitForSelector('#movie_player > .ytc-ccsz input', { state: 'visible', timeout: 3000 });
    assert.equal(parseFloat(await page.locator('.ytc-ccsz input').inputValue()), 1.1);
    await page.evaluate(() => { const r = document.querySelector('.ytc-ccsz input'); r.value = '1.5'; r.dispatchEvent(new Event('input', { bubbles: true })); r.dispatchEvent(new Event('change', { bubbles: true })); });
    for (let i = 0; i < 30 && (await getCfg()).p.cs.s !== 1.5; i++) await page.waitForTimeout(100);
    assert.equal((await getCfg()).p.cs.s, 1.5);
    assert.ok(Math.abs((await box()).font - 20.25 * 1.5) < 0.6);
    // A reload keeps it (and the client player reads the same p.cs).
    await openNative();
    b = await box();
    assert.ok(Math.abs(b.x - 0.2) < 0.003 && Math.abs(b.font - 20.25 * 1.5) < 0.6, JSON.stringify(b));
  });

  await step('native page: 0–9, +/− and V never reach YouTube; typing in a field and other keys do', async () => {
    await page.evaluate(() => { document.activeElement?.blur(); window.__keys = []; });
    await page.mouse.click(1300, 860);
    for (const k of ['5', '0', '9', 'Equal', 'Minus', 'Shift+Equal', 'v', 'Shift+V', 'k', 'ArrowLeft', 'j']) await page.keyboard.press(k);
    assert.deepEqual(await page.evaluate(() => window.__keys.filter(k => k !== 'Shift')), ['k', 'ArrowLeft', 'j']);
    await page.click('#search');
    await page.keyboard.type('a5');
    assert.equal(await page.inputValue('#search'), 'a5');
    assert.deepEqual((await page.evaluate(() => window.__keys)).slice(-2), ['a', '5'], 'typing reaches the page');
  });

  await step('native page: CC follows the client\'s remembered choice (p.cc); a deliberate toggle is remembered', async () => {
    await editCfg({ cc: false });
    await openNative();
    await page.waitForFunction(() => document.querySelector('.ytp-subtitles-button').getAttribute('aria-pressed') === 'false', null, { timeout: 4000 });
    await page.waitForTimeout(1500);
    assert.equal(await page.getAttribute('.ytp-subtitles-button', 'aria-pressed'), 'false', 'switched off once, not toggled back');
    await page.click('.ytp-subtitles-button');
    for (let i = 0; i < 30 && (await getCfg()).p.cc !== true; i++) await page.waitForTimeout(100);
    assert.equal((await getCfg()).p.cc, true, 'turning CC on here is remembered for the client');
    await page.waitForTimeout(1500);
    assert.equal(await page.getAttribute('.ytp-subtitles-button', 'aria-pressed'), 'true');
  });

  await step('native page: the client\'s Shorts switch (shortsInFeeds) keeps Shorts in feeds; the sidebar entry stays out', async () => {
    await setStore({ shortsInFeeds: true });
    await openNative();
    await page.waitForFunction(() => document.documentElement.dataset.ytcShorts === 'on');
    await expectShown(true, ['short-video', 'absolute-short', 'short-shelf']);
    await expectShown(false, ['shorts-nav', 'playable-card']);
    await setStore({ shortsInFeeds: false });
  });

  await step('native page: a related video returns to the client; client pages carry no native cleanup', async () => {
    await openNative();
    await page.click('#related-link');
    await page.waitForURL(u => u.searchParams.get('v') === '-Bxc4gJluC0', { timeout: 8000, waitUntil: 'commit' }).catch(e => { throw new Error(e.message + ' at ' + page.url()); });
    await page.waitForSelector('#ytc-top');
    assert.equal(await page.evaluate(() => document.documentElement.hasAttribute('data-ytc-native')), false);
    await sw.evaluate(o => chrome.storage.local.remove(['ytcConfigV1', 'shortsInFeeds']).then(() => chrome.storage.local.set(Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)))), cfgBefore);
  });

  // ---- actions: comments, chapters, like/dislike, subscribe, save, share, notifications, account
  const calls = ep => log.api.filter(a => a.ep === ep);
  // YTC_SHOTS=dir saves screenshots of the action surfaces (for review).
  await step('comments: lazy load, header, replies, paging to an End marker, timestamps seek, Top/Newest', async () => {
    // The previous step leaves this same video open, whose lazy comment request could land after the log
    // is cleared and count twice: leave that document first.
    await page.goto('about:blank');
    await page.waitForTimeout(100);
    log.api.length = 0;
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-info h1.wt');
    await page.waitForSelector('#ytc-cm .yc-h');
    assert.equal(await page.locator('#ytc-cm .yc-sort button').count(), 2);
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    await page.waitForFunction(() => document.querySelectorAll('#ytc-cm-list > .ycm').length >= 20);
    assert.equal(calls('next').filter(a => a.body.continuation === T_FIRST).length, 1, 'first page token from the next response: ' + JSON.stringify(calls('next').filter(a => a.body.continuation === T_FIRST).map(a => a.headers.referer)));
    assert.equal(await page.locator('#ytc-cm .yc-h h2').textContent(), '585 Comments');
    assert.equal(await page.locator('#ytc-cm-list > .ycm').first().locator('.yca').textContent(), '@EdutainmentGames');
    assert.equal((await page.locator('#ytc-cm-list > .ycm').first().locator('.ycl').textContent()).trim(), '760');
    assert.equal(await page.evaluate(() => window.__xss), undefined);
    assert.ok((await page.locator('#ytc-cm-list > .ycm').nth(1).locator('.yct').textContent()).includes('<img src=x'), 'hostile comment shown as text');
    // Replies on demand, collapsible.
    await page.locator('#ytc-rb-0').click();
    await page.waitForFunction(() => document.querySelectorAll('#ytc-rp-0 .ycm').length === 10);
    assert.equal(calls('next').filter(a => a.body.continuation === T_REP).length, 1);
    assert.equal(await page.locator('#ytc-rp-0 button.ycrb').textContent(), 'Show more replies');
    await shot('comments');
    await page.locator('#ytc-rb-0').click();
    assert.ok(await page.locator('#ytc-rp-0.hide').count(), 'replies hidden again');
    assert.equal(calls('next').filter(a => a.body.continuation === T_REP).length, 1, 'no refetch');
    // Paging until the last page → End (only then).
    await page.evaluate(() => scrollTo(0, document.body.scrollHeight));
    await page.waitForSelector('#ytc-cm-more.yc-end');
    assert.equal(await page.locator('#ytc-cm-more').textContent(), 'End');
    assert.equal(await page.locator('#ytc-cm-list > .ycm').count(), 40);
    assert.ok(calls('next').some(a => a.body.continuation === T_PAGE2));
    // Timestamp in a comment seeks our player.
    await waitT(() => document.documentElement.dataset.ytcFirstFrame);
    await page.locator('#ytc-cm-list > .ycm').nth(1).locator('a[data-act="seek"]').click();
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime - 12) < 2);
    // Newest: list reloads; no End during the transition.
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    const pill = page.evaluate(() => new Promise(res => {
      const b = document.getElementById('ytc-cs1');
      const mo = new MutationObserver(() => { if (b.className !== 'on') return; mo.disconnect(); requestAnimationFrame(() => requestAnimationFrame(() => { const m = document.querySelector('#ytc-csort > i.ycsk'); res({ ok: !!m, run: m ? m.getAnimations().map(a => a.transitionProperty) : [] }); })); });
      mo.observe(b, { attributes: true });
      setTimeout(() => res(null), 3000);
    }));
    await page.locator('#ytc-cm .yc-sort button', { hasText: 'Newest' }).click();
    assert.equal(await page.locator('#ytc-cm-more.yc-end').count(), 0, 'no End while the new sort loads');
    const pr = await pill;
    assert.ok(pr && pr.ok && pr.run.includes('clip-path'), 'the Top/Newest pill slides ' + JSON.stringify(pr));
    await page.waitForTimeout(450);
    const pillOn = await page.evaluate(() => { const s = document.getElementById('ytc-csort'), m = s.querySelector(':scope > i.ycsk'), b = document.getElementById('ytc-cs1'), a = s.getBoundingClientRect(), r = b.getBoundingClientRect(), v = (c => { const v = /inset\(([^)]*?)(?: round [^)]*)?\)/.exec(c)[1].trim().split(/\s+/).map(parseFloat); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; })(getComputedStyle(m).clipPath); return { dl: v[3] - (r.left - a.left), dw: (a.width - v[1] - v[3]) - r.width, dt: v[0] - (r.top - a.top), fg: getComputedStyle(b).color }; });
    assert.ok(Math.abs(pillOn.dl) < 0.6 && Math.abs(pillOn.dw) < 0.6 && Math.abs(pillOn.dt) < 0.6, 'the pill lands exactly on Newest ' + JSON.stringify(pillOn));
    await page.waitForFunction(() => document.querySelector('#ytc-cm-list > .ycm .yca')?.textContent === '@newest-e2e');
    assert.ok(calls('next').some(a => a.body.continuation === T_NEW));
    assert.equal(await page.getAttribute('#ytc-cm .yc-sort button.on', 'aria-pressed'), 'true');
  });

  if (process.env.YTC_SHOTS) await step('shots: watch page body mid-motion (Top/Newest pill, link underline, reply thread unrolling), slowed 10×', async () => {
    const dir = process.env.YTC_SHOTS;
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-cm .yc-sort');
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    await page.waitForSelector('#ytc-cm-list > .ycm');
    await page.waitForTimeout(400);
    await cdp.send('Animation.enable'); await cdp.send('Animation.setPlaybackRate', { playbackRate: 0.1 });
    const hb = await page.locator('#ytc-cm .yc-h').boundingBox();
    await page.locator('#ytc-cm .yc-sort button', { hasText: 'Newest' }).click();
    await page.waitForTimeout(900);
    await page.screenshot({ path: path.join(dir, 'comments-pill-mid.png'), clip: { x: hb.x, y: hb.y - 4, width: 360, height: hb.height + 8 } });
    await page.waitForTimeout(3000);
    await page.screenshot({ path: path.join(dir, 'comments-pill-end.png'), clip: { x: hb.x, y: hb.y - 4, width: 360, height: hb.height + 8 } });
    await page.locator('#ytc-rb-0').click();
    await page.waitForFunction(() => document.querySelectorAll('#ytc-rp-0 .ycm').length > 0);
    await page.locator('#ytc-rb-0').click();
    await page.locator('#ytc-rb-0').click();
    await page.waitForTimeout(1500);
    const rb = await page.locator('#ytc-cm-list > .ycm').first().boundingBox();
    await page.screenshot({ path: path.join(dir, 'comments-replies-unroll-mid.png'), clip: { x: rb.x, y: rb.y, width: 700, height: 420 } });
    await cdp.send('Animation.setPlaybackRate', { playbackRate: 1 });
    await page.evaluate(() => scrollTo(0, 0));
    const a = page.locator('#ytc-desc a').first();
    if (await a.count()) {
      await a.scrollIntoViewIfNeeded();
      await page.waitForTimeout(200);
      await cdp.send('Animation.setPlaybackRate', { playbackRate: 0.1 });
      await a.hover();
      await page.waitForTimeout(1100);
      const db = await page.locator('#ytc-desc').boundingBox();
      await page.screenshot({ path: path.join(dir, 'desc-underline-mid.png'), clip: { x: db.x, y: db.y, width: Math.min(db.width, 760), height: Math.min(db.height, 200) } });
      await cdp.send('Animation.setPlaybackRate', { playbackRate: 1 });
      await page.waitForTimeout(500);
      await page.screenshot({ path: path.join(dir, 'desc-underline-end.png'), clip: { x: db.x, y: db.y, width: Math.min(db.width, 760), height: Math.min(db.height, 200) } });
    }
    await page.mouse.move(5, 5);
  });

  // The chapter panel 160 px below the top bar (room for the list to unfold below it).
  const chapTop = () => page.evaluate(() => { const r = document.getElementById('ytc-chd').getBoundingClientRect(); scrollBy(0, r.top - 160); });
  const chapGeo = () => page.evaluate(() => {
    const q = x => document.querySelector(x), r = x => q(x).getBoundingClientRect(), d = r('#ytc-chd'), c = q('#ytc-chd .ychc'), cr = c.getBoundingClientRect();
    const cl = getComputedStyle(c).clipPath, m = /^inset\((.*?)(?: round [^)]*)?\)$/.exec(cl), parts = m ? m[1].match(/calc\([^)]*\)|\S+/g) : ['0px'];
    const v = parts.map(x => { let t = 0, sg = 1; for (const k of x.replace(/^calc\(|\)$/g, '').split(/\s+/)) { if (k === '-') sg = -1; else if (k === '+') sg = 1; else { t += sg * parseFloat(k) * (k.endsWith('%') ? cr.height / 100 : 1); sg = 1; } } return t; });
    const box = e => { const b = e.getBoundingClientRect(); return { l: b.left, t: b.top, r: b.right, w: b.width, h: b.height, cx: b.left + b.width / 2, cy: b.top + b.height / 2 }; };
    const x = box(q('#ytc-chh .ychx'));
    return { clip: cl, cls: q('#ytc-chd').className, top: d.top, h: d.height, l: d.left, w: d.width, cm: r('#ytc-cm').top, vis: cr.height - (v[2] ?? v[0]) - (v[0] || 0), full: cr.height,
      run: c.getAnimations().length, olOp: getComputedStyle(q('#ytc-chd ol')).opacity, desc: { l: r('#ytc-desc').left, w: r('#ytc-desc').width },
      frame: box(q('#ytc-chh .sbf')), text: { c: x.cy }, chev: box(q('#ytc-chh .ychv')), chevSvg: box(q('#ytc-chh .ychv svg')), row0: box(q('#ytc-chi0 .sbf')) };
  });

  await step('chapters from the player bar are listed and seek our player', async () => {
    await page.goto('https://www.youtube.com/watch?v=kCc8FmEb1nY', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-chap .ych li');
    assert.equal(await page.locator('#ytc-chap li').count(), 30);
    await waitT(() => document.documentElement.dataset.ytcFirstFrame);
    // Compact at rest; the list unfolds under the pointer and a row click still seeks.
    await chapTop();
    assert.equal(await page.getAttribute('#ytc-chd', 'class'), 'ych', 'compact at rest');
    await page.hover('#ytc-chh');
    await waitT(() => document.getElementById('ytc-chd').classList.contains('open'));
    await page.waitForTimeout(500);
    await shot('chapters');
    await page.locator('#ytc-chap li').nth(1).locator('a').click();
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime - 20) < 2);
    await page.mouse.move(700, 880);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
  });

  await step('chapter panel: compact by default and aligned, hover unfolds it over the comments without moving them, pointer-out shrinks faster, a pass-through never opens it, keyboard focus is instant, Escape collapses', async () => {
    await page.mouse.move(700, 880);
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    await chapTop();
    await page.waitForTimeout(300);
    let g = await chapGeo();
    assert.equal(g.cls, 'ych', 'compact: ' + g.cls);
    assert.ok(Math.abs(g.h - 72) < 0.5 && Math.abs(g.vis - 72) < 0.5, `one 72 px row reserved and shown (${g.h}, ${g.vis}, ${g.clip}, ${g.full})`);
    assert.equal(g.olOp, '0', 'the list is folded away');
    // Aligned with the description above (same edges, frame at its text's 17 px inset) and the comments 20 px below.
    assert.ok(Math.abs(g.l - g.desc.l) < 0.5 && Math.abs(g.w - g.desc.w) < 0.5, `same edges as the description (${g.l}/${g.w} vs ${g.desc.l}/${g.desc.w})`);
    assert.ok(Math.abs(g.frame.l - g.l - 17) < 0.5 && Math.abs(g.frame.t - g.top - 9) < 0.5 && g.frame.w === 96 && g.frame.h === 54, 'compact frame 96×54 at 17/9 px ' + JSON.stringify(g.frame));
    assert.ok(Math.abs(g.text.c - (g.top + 36)) <= 1, 'text block centred in the row ' + JSON.stringify(g.text));
    assert.ok(Math.abs(g.chev.cx - g.chevSvg.cx) <= 1 && Math.abs(g.chev.cy - g.chevSvg.cy) <= 1 && Math.abs(g.chev.cy - (g.top + 36)) <= 1, 'chevron centred in its circle and the row ' + JSON.stringify([g.chev, g.chevSvg]));
    assert.ok(Math.abs(g.l + g.w - g.chev.r - 15) < 0.5, 'chevron 15 px from the right edge');
    assert.ok(Math.abs(g.cm - (g.top + 72 + 20)) <= 1, `comments 20 px below (${g.cm - g.top - 72})`);
    // Every chapter shows its own storyboard cell (sheet L2 at 1×, frames every 1 s): the frame taken at its start.
    await waitT(() => !!document.getElementById('ytc-chf2').getAttribute('style'));
    const fr = await page.evaluate(() => [0, 1, 2, 3].map(i => document.getElementById('ytc-chf' + i)).concat(document.getElementById('ytc-chhf')).map(e => {
      const st = e.getAttribute('style'), m = /url\("([^"]+)"\) ([\d.]+)% ([\d.]+)%\/(\d+)% (\d+)%/.exec(st), b = e.parentElement.getBoundingClientRect(), r = e.getBoundingClientRect();
      const u = new URL(m[1]), page = +(/\/M(\d+)\.jpg$/.exec(u.pathname) || [0, 0])[1], cols = +m[4] / 100, rows = +m[5] / 100;
      const col = cols > 1 ? Math.round(+m[2] * (cols - 1) / 100) : 0, row = rows > 1 ? Math.round(+m[3] * (rows - 1) / 100) : 0;
      return { path: u.pathname, sigh: u.searchParams.get('sigh'), n: page * 25 + row * 5 + col, cell: `${page}:${col}:${row}`, fill: [r.left - b.left, r.top - b.top, r.width - b.width, r.height - b.height].every(x => Math.abs(x) < 0.01), st };
    }));
    assert.deepEqual(fr.slice(0, 3).map(f => f.n), [0, 20, 30], 'chapter frames at 0 / 20 / 30 s: ' + JSON.stringify(fr.slice(0, 3)));
    assert.equal(new Set(fr.slice(0, 3).map(f => f.cell)).size, 3, 'three different sprite cells');
    assert.equal(fr[3].n, SECONDS - 1, 'a chapter past the end shows the last frame');
    assert.ok(fr.every(f => /^\/sb\/kCc8FmEb1nY\/storyboard3_L2\/M\d\.jpg$/.test(f.path) && f.sigh === 'rs$E2E2' && f.fill), 'L2 sheets (96 px boxes at 1×), the cell fills its 16:9 box: ' + JSON.stringify(fr));
    assert.equal(fr[4].st, fr[1].st, 'the compact row shows the current chapter\'s frame');
    assert.equal(await page.textContent('#ytc-chht'), await page.textContent('#ytc-chi1 span'));
    assert.equal(await page.textContent('#ytc-chhn'), 'Chapter 2 of 30'); assert.equal(await page.textContent('#ytc-chhc'), '0:20');
    const sheets = [...new Set(log.sb.filter(x => x.url.includes('/kCc8FmEb1nY/')).map(x => new URL(x.url).pathname))].sort();
    assert.deepEqual(sheets, ['/sb/kCc8FmEb1nY/storyboard3_L2/M0.jpg', '/sb/kCc8FmEb1nY/storyboard3_L2/M1.jpg'], 'only the sheets the frames use');
    if (process.env.YTC_SHOTS) {
      const b = await page.locator('#ytc-chd').boundingBox();
      await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'chapters-compact-1400.png'), clip: { x: b.x - 16, y: b.y - 90, width: b.width + 32, height: 260 } });
    }
    // Hover: unfolds over the comments; nothing below moves on any frame, the page does not scroll.
    const sampler = () => page.evaluate(() => { window.__chs = []; const go = () => { if (!window.__chs) return; try { const c = document.querySelector('#ytc-chd .ychc'), r = c.getBoundingClientRect(), H = r.height, cl = getComputedStyle(c).clipPath;
      const m = /^inset\((.*?)(?: round [^)]*)?\)$/.exec(cl), parts = m ? m[1].match(/calc\([^)]*\)|\S+/g) : ['0px'], v = parts.map(x => { let t = 0, sg = 1; for (const k of x.replace(/^calc\(|\)$/g, '').split(/\s+/)) { if (k === '-') sg = -1; else if (k === '+') sg = 1; else { t += sg * parseFloat(k) * (k.endsWith('%') ? H / 100 : 1); sg = 1; } } return t; }), bot = v[2] ?? v[0];
      window.__chs.push({ cm: document.getElementById('ytc-cm').getBoundingClientRect().top, y: scrollY, vis: H - bot - (v[0] || 0) }); } catch (e) { window.__chs.push({ err: String(e) }); } requestAnimationFrame(go); }; requestAnimationFrame(go); });
    const sampled = () => page.evaluate(() => { const s = window.__chs; window.__chs = null; return s; });
    await sampler();
    const hb = await page.locator('#ytc-chh').boundingBox();
    await page.mouse.move(hb.x + 200, hb.y + 30);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych open hv');
    await page.waitForTimeout(650);
    let ss = await sampled();
    g = await chapGeo();
    assert.ok(ss.length > 10 && ss.every(x => x.cm === ss[0].cm && x.y === ss[0].y) && Math.abs(ss[0].cm - g.cm) < 0.01, 'comments and scroll still on every frame of the unfold: ' + JSON.stringify([ss[0], ss.at(-1), ss.length, g]));
    assert.ok(ss.some(x => x.vis > 80 && x.vis < g.vis - 8), 'it unfolded over several frames (not a jump)');
    assert.ok(g.vis > 360 && Math.abs(g.vis - g.full) < 0.5 && g.olOp === '1' && g.run === 0, 'fully open, list shown: ' + JSON.stringify(g));
    assert.ok(Math.abs(g.row0.l - g.frame.l) < 0.5, 'rows\' frames line up with the compact row\'s');
    assert.equal(await page.getAttribute('#ytc-chh', 'aria-expanded'), 'true');
    assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('#ytc-chd .ychc')).transitionDuration), '0.351s', 'opens on the house spring');
    await shot('chapters-open');
    // Pointer out (below the open panel): folds back, faster; the comments never move.
    await sampler();
    await page.mouse.move(hb.x + 200, g.top + g.vis + 40);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
    assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('#ytc-chd .ychc')).transitionDuration), '0.18s', 'closes faster than it opens');
    await page.waitForTimeout(350);
    ss = await sampled();
    assert.ok(ss.every(x => x.cm === ss[0].cm && x.y === ss[0].y), 'comments still while it folds');
    g = await chapGeo();
    assert.ok(Math.abs(g.vis - 72) < 0.5 && g.run === 0, 'back to the compact row');
    // A quick pass across the row never opens it (60 ms intent).
    await sampler();
    await page.mouse.move(hb.x + 200, hb.y - 20); await page.mouse.move(hb.x + 200, hb.y + 30); await page.mouse.move(hb.x + 200, hb.y + 100);
    await page.waitForTimeout(300);
    ss = await sampled();
    assert.ok(ss.every(x => x.vis < 72.5), 'a pass-through does not open it: ' + Math.max(...ss.map(x => x.vis)));
    assert.equal(await page.getAttribute('#ytc-chd', 'class'), 'ych');
    // Mid-unfold, slowed 10×, at 1400 and 2560.
    if (process.env.YTC_SHOTS) {
      await cdp.send('Animation.enable');
      for (const [w, h] of [[1400, 900], [2560, 1440]]) {
        await page.setViewportSize({ width: w, height: h });
        await chapTop();
        await page.waitForTimeout(150);
        const b = await page.locator('#ytc-chd').boundingBox();
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `chapters-compact-${w}.png`), clip: { x: b.x - 16, y: b.y - 90, width: b.width + 32, height: 260 } });
        await cdp.send('Animation.setPlaybackRate', { playbackRate: 0.1 });
        await page.mouse.move(b.x + 200, b.y + 30);
        await page.waitForTimeout(1500); // ≈ 90 ms into the unfold (after the 60 ms intent)
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `chapters-mid-expand-${w}.png`), clip: { x: b.x - 16, y: b.y - 90, width: b.width + 32, height: 560 } });
        await cdp.send('Animation.setPlaybackRate', { playbackRate: 1 });
        await page.waitForTimeout(500);
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `chapters-open-${w}.png`), clip: { x: b.x - 16, y: b.y - 90, width: b.width + 32, height: 560 } });
        await page.mouse.move(b.x + 200, b.y + 560);
        await waitT(() => document.getElementById('ytc-chd').className === 'ych');
        await page.waitForTimeout(300);
      }
      await page.setViewportSize({ width: 1400, height: 900 });
      await chapTop();
    }
    // Keyboard: focus moving in opens it with no motion; Escape folds it at once onto the compact row; Tab opens it again.
    await page.evaluate(() => document.getElementById('ytc-chh').focus());
    assert.equal(await page.getAttribute('#ytc-chd', 'class'), 'ych', 'programmatic focus after a pointer input: stays compact');
    const instant = () => page.evaluate(() => { const c = document.querySelector('#ytc-chd .ychc'); return { cls: document.getElementById('ytc-chd').className, run: c.getAnimations().length + document.querySelector('#ytc-chd ol').getAnimations().length, clip: getComputedStyle(c).clipPath, act: document.activeElement.id || document.activeElement.closest('li')?.id }; });
    await page.keyboard.press('Tab');
    let k = await instant();
    assert.ok(k.cls === 'ych open now' && k.run === 0 && k.clip === 'inset(0px round 12px)' && k.act === 'ytc-chi0', 'Tab into the list: open at once ' + JSON.stringify(k));
    await page.keyboard.press('Escape');
    k = await instant();
    assert.ok(k.cls === 'ych now' && k.run === 0 && /^inset\(0px 0px calc\(100% - 72px\)( 0px)? round 12px\)$/.test(k.clip) && k.act === 'ytc-chh', 'Escape: compact at once, focus on the row ' + JSON.stringify(k));
    assert.equal(await page.getAttribute('#ytc-chh', 'aria-expanded'), 'false');
    // The pointer coming in opens it again even with focus still on the row; leaving folds it.
    await page.mouse.move(hb.x + 200, hb.y + 30);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych open hv');
    await page.mouse.move(hb.x + 200, hb.y - 60);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Shift+Tab'); await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'ytc-chh');
    await page.keyboard.press('Tab');
    k = await instant();
    assert.ok(k.cls === 'ych open now' && k.run === 0 && k.act === 'ytc-chi0', 'Tab on: open again ' + JSON.stringify(k));
    await page.keyboard.press('Shift+Tab'); await page.keyboard.press('Shift+Tab');
    k = await instant();
    assert.ok(k.cls === 'ych now' && k.run === 0 && k.act !== 'ytc-chh', 'focus leaving: compact at once ' + JSON.stringify(k));
    // A click on the open compact row folds it (it stays folded until the pointer leaves); back in, it opens again.
    await page.click('#ytc-chh');
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
    await page.waitForTimeout(150);
    assert.equal(await page.getAttribute('#ytc-chd', 'class'), 'ych', 'stays folded under the pointer');
    await page.mouse.move(hb.x + 200, hb.y - 60);
    await page.mouse.move(hb.x + 200, hb.y + 30);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych open hv');
    await page.mouse.move(hb.x + 200, hb.y - 60);
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
    await page.waitForTimeout(500);
    // The compact row follows playback: the new chapter's text slides in (playback, not a key).
    await page.waitForTimeout(500); // past the last key's no-motion window
    const slid = page.evaluate(() => new Promise(res => { const t = document.getElementById('ytc-chht'); const mo = new MutationObserver(() => { mo.disconnect(); res(document.getElementById('ytc-chhx').getAnimations().length); }); mo.observe(t, { childList: true, characterData: true, subtree: true }); setTimeout(() => res(-1), 5000); }));
    await page.evaluate(() => { document.getElementById('ytc-video').currentTime = 30.5; });
    assert.equal(await slid, 1, 'the compact row\'s text slides to the new chapter');
    await waitT(() => document.getElementById('ytc-chhn').textContent === 'Chapter 3 of 30' && document.getElementById('ytc-chhc').textContent === '0:30');
    assert.equal(await page.textContent('#ytc-chht'), await page.textContent('#ytc-chi2 span'));
    assert.equal(await page.getAttribute('#ytc-chhf', 'style'), await page.getAttribute('#ytc-chf2', 'style'), 'its frame follows');
    await page.evaluate(() => { document.getElementById('ytc-video').currentTime = 20.5; });
    await waitT(() => document.getElementById('ytc-chhn').textContent === 'Chapter 2 of 30');
    await page.evaluate(() => scrollTo(0, 0));
  });

  // Seek-bar geometry: segments at --s..--e of #ytc-segs (whole pixels), a 2 px gap after each but the last.
  const segGeo = () => page.evaluate(() => {
    const b = document.getElementById('ytc-segs').getBoundingClientRect();
    return { l: b.left, w: b.width, segs: [...document.querySelectorAll('#ytc-segs .pbar')].map(e => { const r = e.getBoundingClientRect(); return { l: r.left - b.left, r: r.right - b.left, s: +getComputedStyle(e).getPropertyValue('--s'), e: +getComputedStyle(e).getPropertyValue('--e') }; }) };
  });
  const checkSegs = (g, fr) => {
    assert.equal(g.segs.length, fr.length - 1, 'segment count');
    g.segs.forEach((sg, i) => {
      const gap = i < g.segs.length - 1 ? 2 : 0;
      assert.ok(Math.abs(sg.l - Math.round(fr[i] * g.w)) <= 1, `segment ${i} starts at ${fr[i]} (${sg.l} of ${g.w})`);
      assert.ok(Math.abs(sg.r - (Math.round(fr[i + 1] * g.w) - gap)) <= 1, `segment ${i} ends at ${fr[i + 1]} − gap (${sg.r})`);
      assert.ok(Math.abs(sg.l - Math.round(sg.l)) < 0.01 || Math.abs((g.l + sg.l) - Math.round(g.l + sg.l)) < 0.01, `segment ${i} on a whole pixel (${g.l + sg.l})`);
      if (i > 0) assert.ok(Math.abs(sg.l - g.segs[i - 1].r - 2) < 0.6, `2 px gap before segment ${i} (${sg.l - g.segs[i - 1].r})`);
    });
  };
  const hoverSeek = async f => {
    const r = await page.evaluate(() => { const b = document.getElementById('ytc-segs').getBoundingClientRect(), s = document.getElementById('ytc-seek').getBoundingClientRect(); return { l: b.left, w: b.width, y: s.top + s.height / 2 }; });
    await page.mouse.move(r.l + f * r.w, r.y);
    return r.l + f * r.w;
  };
  const barShot = async name => { if (!process.env.YTC_SHOTS) return; const b = await page.locator('#ytc-player').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, name + '.png'), clip: { x: b.x, y: b.y + b.height - 230, width: b.width, height: 230 } }); };
  const tipBox = () => page.evaluate(() => { const t = document.querySelector('.ptip'), r = t.getBoundingClientRect(), s = document.getElementById('ytc-seek').getBoundingClientRect(); return { l: r.left, r: r.right, c: r.left + r.width / 2, sl: s.left, sr: s.right, op: getComputedStyle(t).opacity, chap: document.getElementById('ytc-tipc').textContent, t: document.getElementById('ytc-tipt').textContent }; });

  await step('chapters split the seek bar: segments, fills, hover tooltip + grown segment, label, list mark, Ctrl/⌘+arrows', async () => {
    await page.evaluate(() => scrollTo(0, 0));
    await waitT(() => document.querySelectorAll('#ytc-segs .pbar').length === 3);
    const titles = await page.$$eval('#ytc-chap li span', xs => xs.map(x => x.textContent));
    assert.equal(await page.getAttribute('#ytc-chb', 'data-n'), '3', 'label shown');
    checkSegs(await segGeo(), [0, 0.5, 0.75, 1]);
    // Paused at 25 s: segment 0 full, segment 1 filled to exactly 25/40 of the whole bar, segment 2 empty.
    await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.pause(); v.currentTime = 25; });
    await waitT(() => Math.abs(parseFloat(document.getElementById('ytc-seek').style.getPropertyValue('--p')) - 0.625) < 0.002);
    await page.waitForTimeout(300);
    const fills = await page.evaluate(() => { const b = document.getElementById('ytc-segs').getBoundingClientRect(); return [...document.querySelectorAll('#ytc-segs .ppos')].map(e => { const r = e.getBoundingClientRect(); return { l: r.left - b.left, r: r.right - b.left, w: b.width }; }); });
    assert.ok(Math.abs(fills[1].r - 0.625 * fills[1].w) < 1, `fill edge at 25 s (${fills[1].r} vs ${0.625 * fills[1].w})`);
    assert.ok(fills[0].r - fills[0].l > 0.49 * fills[0].w && fills[2].r - fills[2].l < 0.5, 'segment 0 full, segment 2 empty');
    assert.equal(await page.textContent('#ytc-chl'), titles[1], 'label = chapter at 25 s');
    assert.equal(await page.getAttribute('#ytc-chi1', 'class'), 'on', 'list marks the current chapter');
    assert.equal(await page.locator('#ytc-chap li.on').count(), 1);
    // Hover: tooltip with the chapter title + time, centered on the pointer; the hovered segment grows.
    const x = await hoverSeek(0.6);
    await waitT(t => document.getElementById('ytc-tipc').textContent === t && document.getElementById('ytc-tipt').textContent === '0:24', titles[1]);
    assert.equal(await page.getAttribute('#ytc-sg1', 'class'), 'pbar on', 'hovered segment grown');
    assert.match(await page.$eval('#ytc-sg1', e => getComputedStyle(e).transform), /^matrix\(1, 0, 0, 1\.6, 0, 0\)$/, 'grown taller than the rest');
    assert.equal(await page.$eval('#ytc-sg0', e => getComputedStyle(e).transform), 'none');
    assert.equal(await page.locator('#ytc-segs .pbar.on').count(), 1);
    let tb = await tipBox();
    assert.equal(tb.op, '1', 'tooltip visible on hover');
    assert.ok(Math.abs(tb.c - x) < 1.5, `tooltip centered on the pointer (${tb.c} vs ${x})`);
    // The storyboard frame at the pointer's time, above the title: 160×90, centred, the right cell (24 s → sheet M0, cell 24).
    const tipFrame = () => page.evaluate(() => {
      const f = document.querySelector('.ptip .ptf'), i = document.getElementById('ytc-tipf'), b = f.getBoundingClientRect(), t = document.getElementById('ytc-tip').getBoundingClientRect(), st = i.getAttribute('style') || '';
      const m = /url\("([^"]+)"\) ([\d.]+)% ([\d.]+)%\/(\d+)% (\d+)%/.exec(st), r = i.getBoundingClientRect();
      if (!m) return { shown: getComputedStyle(f).display };
      const u = new URL(m[1]), page = +(/\/M(\d+)\.jpg$/.exec(u.pathname) || [0, 0])[1], cols = +m[4] / 100, rows = +m[5] / 100;
      const col = cols > 1 ? Math.round(+m[2] * (cols - 1) / 100) : 0, row = rows > 1 ? Math.round(+m[3] * (rows - 1) / 100) : 0;
      return { shown: getComputedStyle(f).display, w: b.width, h: b.height, dc: (b.left + b.width / 2) - (t.left + t.width / 2), above: b.bottom <= document.getElementById('ytc-tipc').getBoundingClientRect().top, n: page * 25 + row * 5 + col, dn: +i.dataset.n, path: u.pathname, fill: Math.abs(r.width - b.width) < 0.01 && Math.abs(r.height - b.height) < 0.01 };
    });
    let tf = await tipFrame();
    assert.ok(tf.shown === 'block' && tf.w === 160 && tf.h === 90 && Math.abs(tf.dc) < 0.6 && tf.above && tf.fill, 'seek preview frame ' + JSON.stringify(tf));
    assert.ok(tf.n === 24 && tf.dn === 24 && tf.path === '/sb/kCc8FmEb1nY/storyboard3_L2/M0.jpg', 'the frame at 0:24 ' + JSON.stringify(tf));
    await barShot('chapters-hover-1400');
    // Near the right end: last chapter, the tooltip stays inside the bar.
    await hoverSeek(0.9);
    await waitT(() => document.getElementById('ytc-tipf').dataset.n === '36');
    tf = await tipFrame();
    assert.ok(tf.n === 36 && tf.path === '/sb/kCc8FmEb1nY/storyboard3_L2/M1.jpg', 'tracks the pointer: 0:36 → the cropped last sheet ' + JSON.stringify(tf));
    await hoverSeek(0.995);
    await waitT(t => document.getElementById('ytc-tipc').textContent === t && document.getElementById('ytc-tipt').textContent === '0:39', titles[2]);
    assert.equal(await page.evaluate(() => document.getElementById('ytc-tipf').dataset.n), '39');
    tb = await tipBox();
    assert.ok(tb.r <= tb.sr + 0.5 && tb.l >= tb.sl - 0.5, `tooltip clamped inside the bar (${tb.l}..${tb.r} in ${tb.sl}..${tb.sr})`);
    assert.equal(await page.getAttribute('#ytc-sg2', 'class'), 'pbar on');
    await hoverSeek(0.002);
    tb = await tipBox();
    assert.ok(tb.l >= tb.sl - 0.5, 'clamped at the left end');
    await page.mouse.move(700, 880);
    await waitT(() => !document.querySelector('#ytc-segs .pbar.on'));
    // Sweeping into another chapter morphs the tooltip's width (its surface clips from the old title's box).
    await hoverSeek(0.3);
    await waitT(t => document.getElementById('ytc-tipc').textContent === t, titles[0]);
    await page.waitForTimeout(400);
    const tipMorph = page.evaluate(() => new Promise(res => {
      const c = document.getElementById('ytc-tipc'), tip = document.getElementById('ytc-tip');
      const mo = new MutationObserver(() => { mo.disconnect(); requestAnimationFrame(() => res({ surf: document.getAnimations().filter(a => a.effect && a.effect.target === tip && a.effect.pseudoElement === '::before').length, clip: getComputedStyle(tip, '::before').clipPath })); });
      mo.observe(c, { childList: true, characterData: true, subtree: true });
      setTimeout(() => res(null), 2000);
    }));
    await hoverSeek(0.6);
    const tm = await tipMorph;
    assert.ok(tm && tm.surf === 1 && /^inset/.test(tm.clip), 'the tooltip morphs between chapter titles ' + JSON.stringify(tm));
    await page.waitForTimeout(400);
    tb = await tipBox();
    assert.ok(Math.abs(tb.c - (await hoverSeek(0.6))) < 1.5, 'still centred on the pointer after the morph');
    await page.mouse.move(700, 880);
    // Playing across a boundary: the label and the list mark follow (no re-render: same elements); the list's
    // highlight is one mark that springs from row 1 to row 2 and lands exactly on it.
    const li = await page.$('#ytc-chi2');
    await chapTop();
    { const hb = await page.locator('#ytc-chh').boundingBox(); await page.mouse.move(hb.x + 200, hb.y + 30); }
    await waitT(() => document.getElementById('ytc-chd').classList.contains('open'));
    await page.waitForTimeout(450);
    const chMark = page.evaluate(() => new Promise(res => {
      const li2 = document.getElementById('ytc-chi2');
      const mo = new MutationObserver(() => { if (li2.className !== 'on') return; mo.disconnect(); requestAnimationFrame(() => requestAnimationFrame(() => { const m = document.querySelector('#ytc-chol > i.ychk'); res({ ok: !!m, run: m ? m.getAnimations().map(a => a.transitionProperty) : [], mk: document.getElementById('ytc-chol').dataset.mk }); })); });
      mo.observe(li2, { attributes: true });
      setTimeout(() => res(null), 7000);
    }));
    await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.currentTime = 29.2; v.play(); });
    await waitT(t => document.getElementById('ytc-chl').textContent === t && document.getElementById('ytc-chi2').className === 'on' && !document.getElementById('ytc-chi1').className, titles[2], 6000);
    assert.ok(await li.evaluate(e => e.isConnected), 'list item kept (class toggle, no re-render)');
    const cm = await chMark;
    assert.ok(cm && cm.ok && cm.mk === '1' && cm.run.includes('clip-path'), 'the chapter highlight slides to the new row ' + JSON.stringify(cm));
    await page.waitForTimeout(550);
    const onRow = () => page.evaluate(() => { const ol = document.getElementById('ytc-chol'), m = ol.querySelector(':scope > i.ychk'), a = ol.querySelector('li.on a'), s = ol.getBoundingClientRect(), r = a.getBoundingClientRect(), v = (c => { const v = /inset\(([^)]*?)(?: round [^)]*)?\)/.exec(c)[1].trim().split(/\s+/).map(parseFloat); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; })(getComputedStyle(m).clipPath); return { dt: v[0] - (r.top - s.top + ol.scrollTop - ol.clientTop), dl: v[3] - (r.left - s.left - ol.clientLeft), dh: (parseFloat(m.style.height) - v[0] - v[2]) - r.height, bg: getComputedStyle(a).backgroundColor, run: m.getAnimations().length }; });
    let orow = await onRow();
    assert.ok(Math.abs(orow.dt) < 0.6 && Math.abs(orow.dl) < 0.6 && Math.abs(orow.dh) < 0.6 && orow.run === 0 && orow.bg === 'rgba(0, 0, 0, 0)', 'the highlight lands exactly on the current row ' + JSON.stringify(orow));
    if (process.env.YTC_SHOTS) {
      await cdp.send('Animation.enable'); await cdp.send('Animation.setPlaybackRate', { playbackRate: 0.1 });
      await page.evaluate(() => { document.getElementById('ytc-video').currentTime = 20.5; });
      await page.waitForTimeout(1300);
      const cb = await page.locator('#ytc-chap').boundingBox();
      await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'chapters-mark-mid.png'), clip: { x: cb.x, y: cb.y, width: cb.width, height: 300 } });
      await cdp.send('Animation.setPlaybackRate', { playbackRate: 1 });
      await page.waitForTimeout(600);
      await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.currentTime = 29.2; v.play(); });
      await waitT(() => document.getElementById('ytc-chi2').className === 'on', null, 6000);
      await page.waitForTimeout(600);
    }
    await page.mouse.move(700, 880);
    await page.evaluate(() => scrollTo(0, 0));
    // Ctrl/⌘+← / →: previous / next chapter (just into chapter 2 → chapter 1's start).
    await page.keyboard.press('Control+ArrowLeft');
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime - 20) < 1);
    await waitT(() => document.getElementById('ytc-chi1').className === 'on');
    assert.equal(await page.evaluate(() => new Promise(res => requestAnimationFrame(() => requestAnimationFrame(() => res(document.querySelector('#ytc-chol > i.ychk').getAnimations().length))))), 0, 'a keyboard chapter jump moves the highlight at once');
    assert.equal(await page.textContent('#ytc-toast'), titles[1], 'toast names the chapter');
    await page.keyboard.press('Meta+ArrowRight');
    await waitT(() => Math.abs(document.getElementById('ytc-video').currentTime - 30) < 1);
    await page.waitForTimeout(400);
    await page.keyboard.press('Control+ArrowRight');
    await page.waitForTimeout(200);
    assert.ok(Math.abs(await page.evaluate(() => document.getElementById('ytc-video').currentTime) - 30) < 2, 'last chapter: Ctrl+→ stays');
    await page.evaluate(() => document.getElementById('ytc-video').pause());
    // The label opens the chapter panel (pinned, scrolled into view); a press elsewhere folds it.
    assert.equal(await page.getAttribute('#ytc-chd', 'class'), 'ych');
    await page.click('#ytc-chb');
    await waitT(() => { const d = document.getElementById('ytc-chd'), r = d.getBoundingClientRect(); return d.className === 'ych open' && r.top >= 0 && r.top < innerHeight; });
    await page.waitForTimeout(450);
    await shot('chapters-list');
    await page.mouse.move(700, 120);
    await page.mouse.down(); await page.mouse.up();
    await waitT(() => document.getElementById('ytc-chd').className === 'ych');
    // 2560 wide: the same fractions on whole pixels.
    await page.setViewportSize({ width: 2560, height: 1440 });
    await page.evaluate(() => scrollTo(0, 0));
    await page.waitForTimeout(200);
    checkSegs(await segGeo(), [0, 0.5, 0.75, 1]);
    await hoverSeek(0.3);
    await waitT(t => document.getElementById('ytc-tipc').textContent === t, titles[0]);
    await barShot('chapters-hover-2560');
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.mouse.move(700, 880);
  });

  await step('description timecodes become chapters when next has none; invalid ones do not', async () => {
    await page.goto('https://www.youtube.com/watch?v=descchaps01', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-chap .ych li');
    assert.deepEqual(await page.$$eval('#ytc-chap li span', xs => xs.map(x => x.textContent)), ['Intro', 'Setup', 'Build https://example.com']);
    await waitT(() => document.querySelectorAll('#ytc-segs .pbar').length === 3);
    checkSegs(await segGeo(), [0, 12 / 40, 26 / 40, 1]);
    await waitT(() => document.getElementById('ytc-chl').textContent !== '');
    await hoverSeek(0.5);
    await waitT(() => document.getElementById('ytc-tipc').textContent === 'Setup' && document.getElementById('ytc-tipt').textContent === '0:20');
    await barShot('chapters-desc');
    await page.mouse.move(700, 880);
    await page.goto('https://www.youtube.com/watch?v=descnochap1', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-cm .yc-h');
    await waitT(() => / \/ 0:40$/.test(document.getElementById('ytc-time').textContent));
    await page.waitForTimeout(300);
    assert.equal(await page.locator('#ytc-chap li').count(), 0, 'no chapter list');
    assert.equal(await page.locator('#ytc-segs .pbar').count(), 1, 'one bar');
    assert.equal(await page.getAttribute('#ytc-chb', 'data-n'), '0', 'no chapter label');
    assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-chb')).display), 'none');
    // Without chapters the tooltip shows the time alone; the single bar spans the whole track.
    const g = await segGeo();
    assert.ok(Math.abs(g.segs[0].l) < 0.5 && Math.abs(g.segs[0].r - g.w) < 1, 'single bar full width');
    await hoverSeek(0.25);
    await waitT(() => document.getElementById('ytc-tipt').textContent === '0:10' && document.getElementById('ytc-tipc').textContent === '');
    assert.equal((await tipBox()).op, '1');
    // No storyboard in this WEB response: the tooltip is the time alone, no frame box.
    assert.deepEqual(await page.evaluate(() => ({ sb: document.getElementById('ytc-player').classList.contains('sb'), f: getComputedStyle(document.querySelector('.ptip .ptf')).display, h: document.getElementById('ytc-tip').getBoundingClientRect().height })), { sb: false, f: 'none', h: 24 }, 'no storyboard: unchanged tooltip');
    assert.equal(await page.locator('#ytc-segs .pbar.on').count(), 0, 'no grown segment without chapters');
    await barShot('nochapters-hover');
    await page.mouse.move(700, 880);
  });

  await step('description: only resource lines (bare links labelled), no hashtags/timestamps/prose, views/date on top, fully open, 12px below the channel row, Block beside links', async () => {
    const shots = async tag => {
      if (!process.env.YTC_SHOTS) return;
      for (const [w, h] of [[1400, 900], [2560, 1440]]) {
        await page.setViewportSize({ width: w, height: h });
        await page.waitForTimeout(150);
        await page.evaluate(() => window.scrollTo(0, 0));
        const r = await page.locator('#ytc-info').boundingBox();
        await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `desc-${tag}-${w}.png`), fullPage: true, clip: { x: 0, y: Math.max(0, r.y - 60), width: w, height: r.height + 120 }, animations: 'disabled' });
      }
      await page.setViewportSize({ width: 1400, height: 900 });
      await page.evaluate(() => window.scrollTo(0, 0));
    };
    for (const [id, tag] of [['descRULES01', 'rules'], ['kCc8FmEb1nY', 'real']]) {
      await page.goto('https://www.youtube.com/watch?v=' + id, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-desc:not(.sk)');
      await shots(tag);
    }
    await page.goto('https://www.youtube.com/watch?v=descRULES01', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-desc:not(.sk)');
    const d = await page.evaluate(() => {
      const desc = document.getElementById('ytc-desc'), cs = getComputedStyle(desc);
      const rows = [...desc.querySelectorAll('.dr')].map(r => { const c = r.cloneNode(true); c.querySelectorAll('.fx-blk').forEach(b => b.remove()); return c.textContent.replace(/\s+/g, ' ').trim(); });
      const ws = desc.firstElementChild, spans = [...ws.children];
      const wo = document.querySelector('#ytc-info .wo').getBoundingClientRect(), box = desc.getBoundingClientRect();
      return {
        rows, hrefs: [...desc.querySelectorAll('.dr a')].map(a => a.getAttribute('href')), first: ws.className, meta: spans.map(s => s.textContent),
        metaGap: spans.length > 1 ? spans[1].getBoundingClientRect().left - spans[0].getBoundingClientRect().right : null,
        metaFont: [...new Set(spans.map(s => getComputedStyle(s).fontSize + '/' + getComputedStyle(s).fontWeight + '/' + getComputedStyle(s).color))],
        text: desc.textContent, seek: desc.querySelectorAll('a[data-act="seek"]').length, more: desc.querySelectorAll('button.lnk,[data-act="desc"]').length,
        clamp: desc.classList.contains('clamp'), open: desc.scrollHeight <= desc.clientHeight + 1 && cs.overflow === 'visible' && cs.maxHeight === 'none',
        gap: box.top - wo.bottom, width: box.width, blk: [...desc.querySelectorAll('.fx-blk')].map(b => b.textContent),
        tags: document.querySelectorAll('#ytc-info a[href^="/hashtag/"]').length,
      };
    });
    assert.deepEqual(d.rows, ['Join my Discord: Discord', 'Project files: Download', 'Related tutorial'], 'only resource lines, bare link labelled: ' + JSON.stringify(d.rows));
    assert.deepEqual(d.hrefs, ['https://discord.example/join', 'https://files.example/dl', '/watch?v=aqz-KE-bpKQ'], 'real destinations');
    assert.ok(!/#|0:30|Intro prose|Thanks|opening/.test(d.text), 'no hashtags, timestamps or prose: ' + d.text);
    assert.equal(d.seek, 0, 'no timestamp links'); assert.equal(d.tags, 0, 'no hashtag links on the watch info');
    assert.equal(d.first, 'ws', 'views/date row first'); assert.equal(d.meta.length, 2); assert.ok(/views/.test(d.meta[0]) && d.meta[1].length > 0, JSON.stringify(d.meta));
    assert.ok(d.metaGap >= 8, 'views and date visibly separated: ' + d.metaGap); assert.equal(d.metaFont.length, 1, 'one typography: ' + d.metaFont);
    assert.ok(!d.more && !d.clamp && d.open, 'fully open, no Show more');
    assert.ok(Math.abs(d.gap - 12) <= 1, '12px below the channel/actions row: ' + d.gap);
    // Spans the column like the title row and the chapter list (no ragged right edge between panels).
    assert.ok(Math.abs(d.width - await page.evaluate(() => document.getElementById('ytc-info').getBoundingClientRect().width)) < 1, 'description spans the column: ' + d.width);
    // Hidden Block buttons take no room: the text after a link continues right after it.
    const gap = await page.evaluate(() => { const a = [...document.querySelectorAll('#ytc-desc a')].find(x => x.nextElementSibling?.classList.contains('fx-blk') && x.nextElementSibling.nextSibling?.textContent?.trim()); if (!a) return null; const r = document.createRange(); r.setStart(a.nextElementSibling.nextSibling, 0); r.setEnd(a.nextElementSibling.nextSibling, 1); const t = r.getBoundingClientRect(), l = a.getBoundingClientRect(); return Math.round(t.left - l.right); });
    assert.ok(gap === null || gap < 8, 'no hole after a link: ' + gap);
    assert.deepEqual(d.blk, ['Block discord.example', 'Block files.example'], 'Block beside external links');
    await page.hover('#ytc-desc .dr a[href="https://files.example/dl"]');
    await page.waitForFunction(() => getComputedStyle(document.querySelectorAll('#ytc-desc .fx-blk')[1]).opacity === '1');
    // A description without any resource keeps just views/date.
    await page.goto('https://www.youtube.com/watch?v=descNONE001', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-desc:not(.sk)');
    const none = await page.evaluate(() => { const d = document.getElementById('ytc-desc'); return { kids: [...d.children].map(c => c.className), visible: d.getBoundingClientRect().height > 0, text: d.textContent }; });
    assert.deepEqual(none.kids, ['ws'], 'views/date only: ' + JSON.stringify(none));
    assert.ok(none.visible && /views/.test(none.text) && !/MOON|today/.test(none.text));
  });

  await step('back to a watch page keeps its comments (memory) and the actions work', async () => {
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-cm .yc-h');
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    await page.locator('#ytc-cm .yc-sort button', { hasText: 'Newest' }).click();
    await page.waitForFunction(() => document.querySelector('#ytc-cm-list > .ycm .yca')?.textContent === '@newest-e2e');
    await page.evaluate(() => document.querySelector('#ytc-list a.t[href^="/watch"]').click());
    await page.waitForFunction(() => new URL(location.href).searchParams.get('v') !== '-Bxc4gJluC0');
    // (Every fixture video shares one comments token: let this page's comments load first.)
    await page.waitForSelector('#ytc-cm .yc-h');
    await page.locator('#ytc-cm').scrollIntoViewIfNeeded();
    await page.waitForSelector('#ytc-cm-list > .ycm');
    const before = calls('next').filter(a => a.body.continuation === T_FIRST || a.body.continuation === T_NEW).length;
    await page.evaluate(() => history.back());
    await page.waitForFunction(() => new URL(location.href).searchParams.get('v') === '-Bxc4gJluC0' && document.querySelector('#ytc-cm-list > .ycm'));
    assert.equal(await page.locator('#ytc-cm-list > .ycm .yca').first().textContent(), '@newest-e2e', 'restored list');
    assert.equal(calls('next').filter(a => a.body.continuation === T_FIRST || a.body.continuation === T_NEW).length, before, 'no refetch');
    await page.locator('#ytc-cm .yc-sort button', { hasText: 'Top' }).click();
    await page.waitForFunction(() => document.querySelector('#ytc-cm-list > .ycm .yca')?.textContent === '@EdutainmentGames');
  });

  await step('signed out: Like asks to sign in, sends nothing; no bell/avatar', async () => {
    log.api.length = 0;
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-like');
    assert.equal(await page.locator('#ytc-like-n').textContent(), '7.4K');
    await page.click('#ytc-like');
    assert.match(await page.locator('#ytc-act-msg').textContent(), /Sign in to rate videos/);
    assert.equal(await page.getAttribute('#ytc-like', 'aria-pressed'), 'false');
    assert.equal(calls('like/like').length, 0);
    assert.equal(await page.locator('#ytc-acct button').count(), 0);
  });

  await ctx.addCookies([{ name: 'SAPISID', value: 'e2e-sapisid', domain: '.youtube.com', path: '/', secure: true }]);
  ACTS.signedIn = true;
  await step('like/dislike: optimistic, exact requests, rollback with reason on failure', async () => {
    log.api.length = 0;
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-like');
    ACTS.delay = 500;
    await page.click('#ytc-like');
    assert.equal(await page.getAttribute('#ytc-like', 'aria-pressed'), 'true', 'pressed before the response');
    await page.waitForFunction(() => document.getElementById('ytc-like').className === 'ya-b on');
    const l = calls('like/like')[0];
    assert.equal(l.body.target.videoId, '-Bxc4gJluC0');
    assert.ok(l.body.params?.startsWith('Cg0KCy1CeGM0'), 'likeParams from the page');
    assert.ok(l.headers.authorization?.startsWith('SAPISIDHASH '), 'signed request');
    ACTS.fail.add('like/dislike');
    await page.click('#ytc-dis');
    assert.equal(await page.getAttribute('#ytc-dis', 'aria-pressed'), 'true');
    assert.equal(await page.getAttribute('#ytc-like', 'aria-pressed'), 'false');
    await page.waitForFunction(() => document.getElementById('ytc-like').getAttribute('aria-pressed') === 'true', null, { timeout: 3000 });
    assert.equal(await page.getAttribute('#ytc-dis', 'aria-pressed'), 'false', 'rolled back to the confirmed Like');
    assert.match(await page.locator('#ytc-act-msg').textContent(), /Rating not saved\. YouTube is unavailable \(HTTP 500\): Backend Error/);
    assert.equal(calls('like/dislike')[0].body.target.videoId, '-Bxc4gJluC0');
    for (let i = 0; i < 50 && !incidents.some(x => x.kind === 'action-like'); i++) await page.waitForTimeout(100);
    const inc = incidents.find(x => x.kind === 'action-like');
    assert.ok(inc && /rating -Bxc4gJluC0: YouTube is unavailable \(HTTP 500\)/.test(inc.reason), 'debug-center incident: ' + JSON.stringify(inc));
    assert.ok(!JSON.stringify(inc).includes('e2e-sapisid'), 'no cookie in the report');
    ACTS.fail.clear();
    await page.click('#ytc-like');
    await page.waitForFunction(() => document.getElementById('ytc-like').className === 'ya-b');
    assert.equal(calls('like/removelike').length, 1);
    ACTS.delay = 0;
  });

  await step('hedged reads: a stalled search / next is sent once more at its deadline, identically; the page renders from the copy, the stalled request is aborted; a slow Like is never sent twice', async () => {
    const failed = [], from = incidents.length;
    const onFail = r => { if (r.url().includes('/youtubei/v1/')) failed.push({ url: r.url(), err: r.failure()?.errorText || '', post: r.postData() || '' }); };
    page.on('requestfailed', onFail);
    try {
      log.api.length = 0;
      HEDGE.stall = { ep: 'search', q: 'hedge stall', n: 1 };
      const t0 = Date.now();
      await page.goto('https://www.youtube.com/results?search_query=hedge+stall', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]', { timeout: 5000 });
      const ms = Date.now() - t0;
      const sent = calls('search').filter(a => a.body.query === 'hedge stall');
      assert.equal(sent.length, 2, 'the stalled search went out once more (not more)');
      assert.equal(JSON.stringify(sent[1].body), JSON.stringify(sent[0].body), 'identical body (same context, visitor, query)');
      assert.equal(sent[1].headers.authorization, sent[0].headers.authorization, 'same session');
      const gap = sent[1].t - sent[0].t;
      assert.ok(gap >= 1000 && gap < 1700, `copy at the search deadline (1.2 s): ${gap} ms`);
      assert.ok(ms < 2500, `rendered from the copy: ${ms} ms`);
      for (let i = 0; i < 30 && !failed.some(f => f.post.includes('hedge stall')); i++) await page.waitForTimeout(100);
      assert.ok(failed.some(f => f.post.includes('hedge stall') && /ABORTED/.test(f.err)), 'the stalled request was aborted: ' + JSON.stringify(failed.map(f => f.err)));
      // A watch page whose `next` stalls: the copy fills it.
      log.api.length = 0;
      HEDGE.stall = { ep: 'next', v: '-Bxc4gJluC0', n: 1 };
      await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-like', { timeout: 5000 });
      const nx = calls('next').filter(a => a.body.videoId === '-Bxc4gJluC0' && !a.body.continuation && !a.url.includes('fields='));
      assert.equal(nx.length, 2, 'next hedged once');
      assert.equal(JSON.stringify(nx[1].body), JSON.stringify(nx[0].body));
      // Home's first page belongs to the Home path (src/app/homeready.rs), which sends its own single hedge:
      // the generic hedger adds nothing on top (no ready response in this check).
      log.api.length = 0;
      await page.evaluate(() => { localStorage.setItem('ytc-test-noready', '1'); sessionStorage.removeItem('ytc:ready'); });
      HEDGE.stall = { ep: 'browse', home: true, n: 1 };
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForTimeout(3200);
      assert.equal(calls('browse').filter(a => a.body.browseId === 'FEwhat_to_watch' && !a.body.continuation).length, 2, 'Home browse: the first request + the Home path\'s one hedge, nothing more');
      await page.evaluate(() => localStorage.removeItem('ytc-test-noready'));
      HEDGE.stall = null;
      await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-like');
      // Actions are never hedged: a Like slower than every deadline goes out once.
      log.api.length = 0;
      ACTS.delay = 2600;
      await page.click('#ytc-like');
      await page.waitForTimeout(3200);
      assert.equal(calls('like/like').length + calls('like/removelike').length, 1, 'one rating request');
      ACTS.delay = 0;
      if (DEV_BUILD) {
        await page.goto('https://www.youtube.com/results?search_query=after+hedge', { waitUntil: 'commit' });
        for (let i = 0; i < 50 && !telBatches(from).some(b => b.events.some(e => e.m === 'req.next' && e.hedge_fired)); i++) await page.waitForTimeout(100);
        const ev = telBatches(from).flatMap(b => b.events);
        const nev = ev.find(e => e.m === 'req.next' && e.hedge_fired);
        assert.ok(nev && nev.hedge_won === true && nev.path === 'hedge' && nev.status === 200 && nev.v >= 1000, 'req.next records the hedge: ' + JSON.stringify(nev));
        const nav = ev.find(e => e.m === 'nav.watch' && e.reqs?.some(r => r.hedge_won));
        assert.ok(nav, 'the navigation breakdown names the hedged request');
      }
    } finally {
      HEDGE.stall = null; ACTS.delay = 0; page.off('requestfailed', onFail);
      // The next steps start on this watch page.
      await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-sub button');
    }
  });

  await step('subscribe/unsubscribe: channelIds, deliberate unsubscribe, rollback on failure', async () => {
    log.api.length = 0;
    await page.click('#ytc-sub button');
    assert.equal((await page.locator('#ytc-sub button').textContent()), 'Subscribed');
    await page.waitForFunction(() => !document.querySelector('#ytc-sub .busy'));
    assert.deepEqual(calls('subscription/subscribe')[0].body.channelIds, ['UCaZO-66UcqyVT9btiqdDryA']);
    await page.click('#ytc-sub button');
    assert.equal(await page.locator('#ytc-sub button').textContent(), 'Unsubscribe?');
    assert.equal(calls('subscription/unsubscribe').length, 0, 'first click only asks');
    await page.click('#ytc-sub button');
    await page.waitForFunction(() => document.querySelector('#ytc-sub button').textContent === 'Subscribe' && !document.querySelector('#ytc-sub .busy'));
    assert.deepEqual(calls('subscription/unsubscribe')[0].body.channelIds, ['UCaZO-66UcqyVT9btiqdDryA']);
    ACTS.fail.add('subscription/subscribe');
    await page.click('#ytc-sub button');
    await page.waitForFunction(() => document.querySelector('#ytc-sub button').textContent === 'Subscribe' && !document.querySelector('#ytc-sub .busy'));
    assert.match(await page.locator('#ytc-act-msg').textContent(), /Subscription not saved/);
    ACTS.fail.clear();
  });

  await step('save: hover opens after intent, hover selects nothing, toggles/create/rollback, closes on leave', async () => {
    // Signed in, the membership is asked once quietly after the page settled: the bookmark is filled
    // (mint) because the video is in Watch later, without opening the chooser.
    await page.waitForFunction(() => document.getElementById('ytc-save-b')?.classList.contains('saved'), null, { timeout: 4000 });
    assert.equal(await page.locator('#ytc-save-l').textContent(), 'Saved');
    assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('#ytc-save-b .ya-bm svg.f')).fill), 'rgb(183, 235, 202)', 'mint fill');
    log.api.length = 0;
    await page.mouse.move(5, 5);
    await page.hover('#ytc-save button[data-act="a-save"]');
    await page.waitForSelector('#ytc-save-p .ya-opt');
    assert.equal(calls('playlist/get_add_to_playlist').length, 0, 'membership from the quiet check, no new request');
    assert.equal(await page.getAttribute('#ytc-pl-0', 'aria-checked'), 'true', 'WL contains the video');
    assert.equal(await page.getAttribute('#ytc-pl-1', 'aria-checked'), 'false');
    assert.ok((await page.locator('#ytc-pl-1').textContent()).includes('<img src=x'), 'title escaped');
    await page.hover('#ytc-pl-1'); await page.hover('#ytc-pl-0'); await page.waitForTimeout(150);
    assert.equal(calls('browse/edit_playlist').length, 0, 'hover selects nothing');
    await page.click('#ytc-pl-1');
    assert.equal(await page.getAttribute('#ytc-pl-1', 'aria-checked'), 'true');
    assert.ok((await page.evaluate(() => document.querySelector('#ytc-pl-1 .ya-ck svg').getAnimations().map(a => a.transitionProperty))).includes('stroke-dashoffset'), 'the check mark draws itself in');
    await page.waitForFunction(() => !document.querySelector('#ytc-save-p .busy'));
    assert.deepEqual(calls('browse/edit_playlist')[0].body.actions, [{ addedVideoId: '-Bxc4gJluC0', action: 'ACTION_ADD_VIDEO' }]);
    assert.equal(calls('browse/edit_playlist')[0].body.playlistId, 'PLe2e');
    ACTS.fail.add('browse/edit_playlist'); ACTS.delay = 400;
    await page.click('#ytc-pl-0');
    assert.equal(await page.getAttribute('#ytc-pl-0', 'aria-checked'), 'false', 'optimistic remove');
    await page.waitForFunction(() => document.getElementById('ytc-pl-0').getAttribute('aria-checked') === 'true');
    assert.match(await page.locator('#ytc-save-m').textContent(), /Removing not saved/);
    for (let i = 0; i < 50 && !incidents.some(x => x.kind === 'action-save'); i++) await page.waitForTimeout(100);
    assert.ok(incidents.some(x => x.kind === 'action-save' && /edit_playlist remove -Bxc4gJluC0/.test(x.reason)), 'save incident');
    assert.deepEqual(calls('browse/edit_playlist')[1].body.actions, [{ action: 'ACTION_REMOVE_VIDEO_BY_VIDEO_ID', removedVideoId: '-Bxc4gJluC0' }]);
    ACTS.fail.clear(); ACTS.delay = 0;
    // Optimistic bookmark: removing the video from every playlist empties it at once (before the
    // responses), adding one back fills it again.
    ACTS.delay = 400;
    for (const i of [0, 1]) await page.click(`#ytc-pl-${i}`);
    assert.ok(!(await page.locator('#ytc-save-b').getAttribute('class')).includes('saved'), 'emptied at once');
    assert.equal(await page.locator('#ytc-save-l').textContent(), 'Save');
    await shot('save-empty');
    await page.click('#ytc-pl-1');
    assert.ok((await page.locator('#ytc-save-b').getAttribute('class')).includes('saved'), 'filled at once');
    assert.ok((await page.evaluate(() => document.querySelector('#ytc-save-b .ya-bm svg.f').getAnimations().map(a => a.transitionProperty))).includes('clip-path'), 'the fill sweeps in (pointer)');
    await page.waitForFunction(() => !document.querySelector('#ytc-save-p .busy'));
    ACTS.delay = 0;
    await page.click('#ytc-pl-0');
    await page.waitForFunction(() => !document.querySelector('#ytc-save-p .busy'));
    await page.fill('#ytc-a-newpl input', 'My list');
    await page.press('#ytc-a-newpl input', 'Enter');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-save-p .ya-opt').length === 3);
    await shot('save');
    assert.deepEqual(calls('playlist/create')[0].body.videoIds, ['-Bxc4gJluC0']);
    assert.equal(calls('playlist/create')[0].body.title, 'My list');
    assert.equal(calls('playlist/create')[0].body.privacyStatus, 'PRIVATE');
    assert.equal(await page.getAttribute('#ytc-pl-0', 'aria-checked'), 'true');
    await page.mouse.move(5, 400);
    await page.waitForSelector('#ytc-save-p', { state: 'hidden', timeout: 1500 });
    // Click opens (keyboard path), Escape closes.
    await page.focus('#ytc-save button[data-act="a-save"]');
    await page.keyboard.press('Enter');
    await page.waitForSelector('#ytc-save-p');
    await page.keyboard.press('Escape');
    await page.waitForSelector('#ytc-save-p', { state: 'hidden' });
    assert.equal(calls('playlist/get_add_to_playlist').length, 0, 'membership kept in memory');
    assert.ok((await page.locator('#ytc-save-b').getAttribute('class')).includes('saved'));
    await shot('save-saved');
  });

  await step('share: youtu.be link, optional start time', async () => {
    await page.evaluate(() => { const v = document.getElementById('ytc-video'); v.pause(); v.currentTime = 15; });
    await page.waitForTimeout(100);
    await page.click('#ytc-share button[data-act="a-share"]');
    assert.equal(await page.inputValue('#ytc-share-u'), 'https://youtu.be/-Bxc4gJluC0');
    await page.click('#ytc-share-t');
    assert.match(await page.inputValue('#ytc-share-u'), /^https:\/\/youtu\.be\/-Bxc4gJluC0\?t=1[45]$/);
    await shot('share');
    await page.click('#ytc-copy');
    assert.match(await page.getAttribute('#ytc-copy', 'class'), /\bdone\b/, 'Copy morphs into its confirmation');
    assert.match(await page.locator('#ytc-copied').textContent(), /Copied|⌘C/);
    assert.match(await page.getAttribute('#ytc-copy', 'aria-label'), /copied|⌘C/);
    await page.mouse.click(5, 400);
    await page.waitForSelector('#ytc-share-p', { state: 'hidden' });
  });

  await step('notifications: count from topbar, list, More, mark read on click; account menu', async () => {
    log.api.length = 0;
    await page.waitForSelector('#ytc-acct .yac-n');
    assert.equal(await page.locator('#ytc-acct .yac-n').textContent(), '3');
    assert.equal(await page.getAttribute('#ytc-acct .yac-av img', 'src'), 'https://yt3.ggpht.com/e2e-avatar=s88');
    await page.click('#ytc-acct button[data-act="a-nt"]');
    await page.waitForSelector('#ytc-nt-p .yac-i');
    assert.equal(calls('notification/get_notification_menu')[0].body.notificationsMenuRequestType, 'NOTIFICATIONS_MENU_REQUEST_TYPE_INBOX');
    assert.equal(await page.locator('#ytc-acct .yac-n:not(.z)').count(), 0, 'badge cleared on open');
    assert.equal(await page.locator('#ytc-nt-p .yac-i').count(), 2);
    assert.equal(await page.evaluate(() => window.__xss), undefined);
    await page.click('#ytc-nt-p button[data-act="a-ntmore"]');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-nt-p .yac-i').length === 3);
    assert.equal(calls('notification/get_notification_menu')[1].body.ctoken, 'CTOK');
    await shot('notifications');
    const docs = log.docs.length;
    await page.click('#ytc-nt-p .yac-i >> nth=1');
    await page.waitForSelector('#ytc-nt-p', { state: 'hidden' });
    assert.equal(calls('notification/record_interactions')[0].body.serializedRecordNotificationInteractionsRequest, 'SER2');
    assert.equal(log.docs.length, docs, 'in-app navigation');
    await page.click('#ytc-acct button[data-act="a-ac"]');
    await page.waitForFunction(() => document.querySelector('#ytc-ac-p')?.textContent.includes('E2E Viewer'));
    assert.equal(calls('account/account_menu').length, 1);
    assert.equal(await page.getAttribute('#ytc-ac-p a[data-arg="/logout"]', 'href'), '/logout');
    // Trimmed account menu: exactly these four entries, in this order.
    assert.deepEqual(await page.$$eval('#ytc-ac-p .yac-a', as => as.map(a => a.textContent.trim())), ['Google Account', 'Switch account', 'Settings', 'Sign out']);
    await shot('account-menu');
    await page.mouse.click(600, 600);
    await page.waitForSelector('#ytc-ac-p', { state: 'hidden' });
  });
  // Routing (above) disables Chromium's HTTP cache, so the cached-thumbnail promise of the
  // fade_imgs primitive is checked here: bridge.js alone in a plain browser, a local server
  // with YouTube's image cache headers.
  await step('fade_imgs: a late image fades in; a cached one (srcset, same document) is never hidden, not even for one frame', async () => {
    const http = require('node:http');
    const srv = http.createServer((req, res) => {
      if (req.url === '/') { res.writeHead(200, { 'content-type': 'text/html' }); return res.end('<!doctype html><style>img.ld{opacity:0}img.ldf{transition:opacity 220ms}</style><div id="box"></div><script src="/bridge.js"></script>'); }
      if (req.url === '/bridge.js') { res.writeHead(200, { 'content-type': 'text/javascript' }); return res.end(fs.readFileSync(path.join(__dirname, '..', 'browser', 'bridge.js'))); }
      setTimeout(() => { res.writeHead(200, { 'content-type': 'image/png', 'cache-control': 'public, max-age=7200' }); res.end(PNG); }, 250);
    }).listen(0, '127.0.0.1');
    await new Promise(r => srv.once('listening', r));
    const browser = await chromium.launch({ headless: true, ...chromiumOptions() });
    try {
      const p = await browser.newPage();
      await p.goto(`http://127.0.0.1:${srv.address().port}/`);
      const html = [1, 2, 3, 4].map(i => `<img src="/i${i}.png" srcset="/i${i}.png 320w, /j${i}.png 720w" sizes="300px" width="160" height="90">`).join('');
      // Fresh: hidden while loading (from the first frame), then faded in.
      const first = await p.evaluate(h => new Promise(r => { const box = document.getElementById('box'); box.innerHTML = h; YTC.fade_imgs('#box img'); requestAnimationFrame(() => r([...box.querySelectorAll('img')].map(x => x.className))); }), html);
      assert.deepEqual(first, ['ld', 'ld', 'ld', 'ld']);
      await p.waitForFunction(() => [...document.querySelectorAll('#box img')].every(x => x.className === 'ldf'), null, { timeout: 3000 });
      // The same images again: complete once their source is chosen, so never hidden.
      const again = await p.evaluate(h => new Promise(r => { const box = document.getElementById('box'); box.innerHTML = h; YTC.fade_imgs('#box img'); requestAnimationFrame(() => r([...box.querySelectorAll('img')].map(x => [x.className, x.complete]))); }), html);
      assert.deepEqual(again, [['', true], ['', true], ['', true], ['', true]]);
    } finally {
      await browser.close();
      srv.close();
    }
  });

  await step('motion: popovers grow from their trigger and work on the frame they open (keyboard: no motion); hover-out and release restore; late thumbnails fade in', async () => {
    // Animations slowed 20× so the checks land mid-transition deterministically.
    const slow = rate => cdp.send('Animation.setPlaybackRate', { playbackRate: rate });
    await cdp.send('Animation.enable');
    // The panel never moves (rows land where they stay); its surface (::before) morphs out of the trigger.
    const pop = id => page.evaluate(id => {
      const p = document.getElementById(id), cs = getComputedStyle(p), up = cs.getPropertyValue('--up').trim();
      return { cls: p.className, vis: cs.visibility, pe: cs.pointerEvents, op: +cs.opacity, tf: cs.transform, clip: getComputedStyle(p, '::before').clipPath, open: `inset(${up} 0px 0px round 12px)`, focus: document.activeElement && document.activeElement.id };
    }, id);
    const hits = (sel, inside) => page.evaluate(([sel, inside]) => {
      const r = document.querySelector(sel).getBoundingClientRect(), h = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return !!h && !!h.closest(inside);
    }, [sel, inside]);
    try {
      // Share (mouse): visible, hit-testable and focused at once, while the entrance still runs.
      await page.mouse.move(5, 400);
      await slow(0.05);
      await page.click('#ytc-share-b');
      let s = await pop('ytc-share-p');
      assert.equal(s.cls, 'ya-pop open in');
      assert.deepEqual([s.vis, s.pe, s.focus, s.tf], ['visible', 'auto', 'ytc-share-u', 'none']);
      assert.ok(s.op < 1 && s.clip !== s.open, `entrance running (opacity ${s.op}, surface ${s.clip})`);
      assert.ok(await hits('#ytc-copy', '#ytc-copy'), 'Copy is clickable on the opening frame');
      if (process.env.YTC_SHOTS) { const b = await page.locator('#ytc-acts').boundingBox(); for (const [ms, n] of [[300, 1], [600, 2], [900, 3], [2000, 4]]) { await page.waitForTimeout(ms); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `motion-share-opening-${n}.png`), clip: { x: b.x + b.width - 520, y: b.y - 10, width: 540, height: 170 } }); } }
      await slow(1);
      await page.waitForTimeout(300);
      // It grows out of the trigger: the surface starts as the Share button's own box and radius.
      s = await pop('ytc-share-p');
      assert.deepEqual([s.tf, s.clip], ['none', s.open], 'settled: the full 12 px panel');
      const geo = await page.evaluate(() => {
        const p = document.getElementById('ytc-share-p'), b = document.getElementById('ytc-share-b'), v = k => parseFloat(getComputedStyle(p).getPropertyValue(k));
        return { p: p.getBoundingClientRect().toJSON(), b: b.getBoundingClientRect().toJSON(), bw: v('--bw'), bh: v('--bh'), up: v('--up'), bx: v('--bx'), br: v('--br') };
      });
      const from = { left: geo.p.right - geo.bx - geo.bw, top: geo.p.top - geo.up, right: geo.p.right - geo.bx, bottom: geo.p.top - geo.up + geo.bh };
      for (const k of ['left', 'top', 'right', 'bottom']) assert.ok(Math.abs(from[k] - geo.b[k]) <= 0.5, `morph starts at the button (${k}): ${JSON.stringify(from)} vs ${JSON.stringify(geo.b)}`);
      assert.equal(geo.br, geo.b.height / 2, 'with the button\'s radius');
      assert.ok(Math.abs(geo.p.right - geo.b.right) <= 1 && Math.abs(geo.p.top - geo.b.bottom - 6) <= 1, 'panel 6 px below, right edges aligned');
      // Outside click: no longer clickable at once; the exit (faster) finishes, then it is hidden.
      await slow(0.05);
      await page.mouse.click(5, 400);
      s = await pop('ytc-share-p');
      assert.deepEqual([s.cls, s.pe, s.vis, s.tf], ['ya-pop', 'none', 'visible', 'none']);
      assert.ok(!(await hits('#ytc-copy', '#ytc-share-p')), 'a closing panel takes no clicks');
      if (process.env.YTC_SHOTS) { await page.waitForTimeout(500); const b = await page.locator('#ytc-acts').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'motion-share-closing.png'), clip: { x: b.x + b.width - 520, y: b.y - 10, width: 540, height: 150 } }); }
      await slow(1);
      await page.waitForSelector('#ytc-share-p', { state: 'hidden', timeout: 1000 });
      // Keyboard: the Save chooser opens and closes without motion, even with animations slowed.
      await slow(0.05);
      await page.focus('#ytc-save-b');
      await page.keyboard.press('Enter');
      s = await pop('ytc-save-p');
      assert.ok(/\bopen\b/.test(s.cls) && /\bnow\b/.test(s.cls) && !/\bin\b/.test(s.cls), s.cls);
      assert.deepEqual([s.vis, s.op, s.tf, s.clip], ['visible', 1, 'none', s.open]);
      await page.keyboard.press('Escape');
      s = await pop('ytc-save-p');
      assert.deepEqual([s.vis, s.focus], ['hidden', 'ytc-save-b']);
      // Bell (mouse): a row is clickable on the opening frame; it grows from the bell.
      await page.click('#ytc-nt-b');
      s = await pop('ytc-nt-p');
      assert.deepEqual([s.cls, s.vis, s.pe, s.tf], ['ya-pop yac-p open in', 'visible', 'auto', 'none']);
      assert.ok(s.op < 1 && s.clip !== s.open, 'bell panel entrance running');
      if (process.env.YTC_SHOTS) for (const [ms, n] of [[300, 1], [600, 2], [900, 3], [2000, 4]]) { await page.waitForTimeout(ms); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `motion-bell-opening-${n}.png`), clip: { x: 900, y: 0, width: 500, height: 360 } }); }
      await page.waitForSelector('#ytc-nt-p .yac-i');
      assert.ok(await hits('#ytc-nt-p .yac-i', '#ytc-nt-p'), 'rows take clicks while it opens');
      await page.keyboard.press('Escape');
      assert.equal((await pop('ytc-nt-p')).vis, 'hidden', 'Escape closes at once');
      await slow(1);

      // Cards: hover ring at once, eased off; the press gives (.985) and the release restores.
      await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c a.th img');
      const th = page.locator('#ytc-list .c a.th').nth(1);
      const ring = () => th.evaluate(el => getComputedStyle(el).outlineColor);
      await th.hover();
      assert.equal(await ring(), 'rgb(41, 57, 54)', 'ring on at once');
      await slow(0.05);
      await page.mouse.move(5, 600);
      await page.waitForTimeout(300);
      const mid = await ring();
      assert.ok(mid !== 'rgb(41, 57, 54)' && mid !== 'rgba(0, 0, 0, 0)', `ring eases off (${mid})`);
      await slow(1);
      await page.waitForFunction(() => getComputedStyle(document.querySelectorAll('#ytc-list .c a.th')[1]).outlineColor === 'rgba(0, 0, 0, 0)', null, { timeout: 1000 });
      const b = await th.boundingBox();
      await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
      await page.mouse.down();
      await page.waitForTimeout(200);
      assert.equal(await th.evaluate(el => getComputedStyle(el).transform), 'matrix(0.985, 0, 0, 0.985, 0, 0)');
      const url = page.url();
      await page.mouse.move(5, 600);
      await page.mouse.up(); // released outside: no click
      await page.waitForTimeout(250);
      assert.equal(await th.evaluate(el => getComputedStyle(el).transform), 'none', 'release restores');
      assert.equal(page.url(), url);
      // Filters panel: hover opens after the intent, leaving closes it back to its resting scale.
      await page.hover('#ytc-hfx .fx-fb');
      await page.waitForTimeout(450); // 40 ms intent + the UI spring's settle (--d-spring 351 ms)
      assert.deepEqual(await page.$eval('#ytc-hfx .fx-p', p => [getComputedStyle(p).visibility, getComputedStyle(p).transform]), ['visible', 'none']);
      await page.mouse.move(5, 600);
      await page.waitForTimeout(450);
      // Closed, it rests raised onto its button (37 px + the 6 px gap), clipped to a button-sized box: it morphs out of the button.
      assert.deepEqual(await page.$eval('#ytc-hfx .fx-p', p => [getComputedStyle(p).visibility, getComputedStyle(p).transform, /^inset\(0px 0px calc\(-37px \+ 100%\) calc\(-112px \+ 100%\) round 10px\)$|^inset\(0px 0px calc\(100% - 37px\) calc\(100% - 112px\) round 10px\)$/.test(getComputedStyle(p).clipPath) || getComputedStyle(p).clipPath]), ['hidden', 'matrix(1, 0, 0, 1, 0, -43)', true]);

      // Skeleton → results: every placeholder sits where its content lands (thumbnails, title and
      // metadata lines), so nothing below moves when the cards replace it.
      IMG.search = 700;
      await page.goto('https://www.youtube.com/results?search_query=roblox+skeleton', { waitUntil: 'commit' });
      await page.waitForSelector('.c.sk');
      const boxes = () => page.$$eval('.grid > .c', cs => cs.slice(0, 8).map(c => {
        const r = el => { const b = el.getBoundingClientRect(); return [Math.round(b.top), Math.round(b.left), Math.round(b.height)]; };
        return { th: r(c.querySelector('.th')), t: r(c.querySelector('.t')), m: [...c.querySelectorAll('.m')].map(r) };
      }));
      const sk = await boxes();
      IMG.search = 0;
      await page.waitForSelector('#ytc-list .c:not(.sk)');
      const real = await boxes();
      for (let i = 0; i < 8; i++) {
        assert.deepEqual(real[i].th, sk[i].th, `card ${i} thumbnail`);
        if (real[i].t[2] === 48) assert.deepEqual(real[i].t, sk[i].t, `card ${i} title lines`);
      }
      if (real[0].t[2] === 48 && real[0].m.length === 2) assert.deepEqual(real[0].m, sk[0].m, 'metadata lines');
      // Thumbnails: late ones are hidden until they land, then fade in; nothing is left hidden.
      IMG.delay = 300;
      await page.goto('https://www.youtube.com/results?search_query=roblox+fade', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c a.th img');
      assert.ok(await page.locator('#ytc-list img.ld').count() > 0, 'loading thumbnails wait hidden');
      if (process.env.YTC_SHOTS) { await slow(0.1); await page.waitForSelector('#ytc-list img.ldf', { timeout: 3000 }); await page.waitForTimeout(150); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'motion-thumbs-fading.png'), clip: { x: 0, y: 60, width: 1400, height: 420 } }); await slow(1); }
      // (Lazy ones below the fold load, and fade, when scrolled near.)
      await page.waitForFunction(() => !document.querySelector('#ytc-list img.ld:not([loading=lazy])'), null, { timeout: 4000 });
      assert.ok(await page.locator('#ytc-list img.ldf').count() > 0, 'late thumbnails fade');
      await page.waitForTimeout(400);
      assert.deepEqual([...new Set(await page.$$eval('#ytc-list .c a.th img', xs => xs.filter(x => x.complete).map(x => getComputedStyle(x).opacity)))], ['1']);
    } finally {
      IMG.delay = 0; IMG.search = 0;
      await slow(1).catch(() => {});
    }
  });

  // ---- chrome motion (header, search, sidebar, bell): pointer morphs, keyboard/back instant, typing untouched
  const slowAnims = rate => cdp.send('Animation.setPlaybackRate', { playbackRate: rate });
  const running = sel => page.evaluate(sel => document.getAnimations().filter(a => a.playState === 'running' && a.effect && a.effect.target && a.effect.target.closest && a.effect.target.closest(sel))
    .map(a => (a.transitionProperty || a.animationName) + (a.effect.pseudoElement || '')), sel);
  const shotAt = async (name, clip, steps) => { if (!process.env.YTC_SHOTS) return; for (const [ms, n] of steps) { await page.waitForTimeout(ms); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, `${name}-${n}.png`), clip }); } };
  await step('motion: search field widens on pointer focus (spring), the first keystroke paints on its keydown frame, the dropdown unfolds with rows clickable at once; × clears; keyboard focus and Enter are instant', async () => {
    await cdp.send('Animation.enable');
    try {
      await page.goto('https://www.youtube.com/results?search_query=roblox', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c');
      // Recent searches for the dropdown: on the "roblox morph" page, "roblox morph lua" matches.
      for (const q of ['roblox morph lua', 'roblox morph']) { await page.fill('#ytc-q', q); await page.press('#ytc-q', 'Enter'); await page.waitForFunction(q => new URLSearchParams(location.search).get('search_query') === q, q); }
      await page.waitForSelector('#ytc-list .c');
      await page.mouse.click(5, 700);
      const box = () => page.evaluate(() => { const r = document.getElementById('ytc-q').getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; });
      const q0 = await box();
      await page.evaluate(() => { window.__f = 0; const t = () => { window.__f++; requestAnimationFrame(t); }; requestAnimationFrame(t);
        const q = document.getElementById('ytc-q'); q.addEventListener('keydown', () => { window.__kd = window.__f; }, { once: true }); q.addEventListener('input', () => { window.__in = window.__f; window.__val = q.value; }, { once: true }); });
      await slowAnims(0.05);
      await page.mouse.click(q0[0] + 200, q0[1] + q0[3] / 2);
      const st = await page.evaluate(() => ({ cls: document.getElementById('ytc-search').className, sug: document.getElementById('ytc-sug').className, focus: document.activeElement.id }));
      assert.deepEqual(st, { cls: '', sug: 'sug on', focus: 'ytc-q' });
      const a1 = await running('#ytc-search');
      assert.ok(['clip-path::before', 'clip-path::after', 'translate'].every(x => a1.includes(x)), 'field morph running: ' + a1);
      assert.ok(a1.includes('ytcglance'), 'the magnifier glances: ' + a1);
      assert.ok(a1.filter(x => x === 'opacity').length >= 2 && a1.includes('clip-path::before'), 'dropdown unfolding, rows settling: ' + a1);
      assert.ok(await page.evaluate(() => { const r = document.querySelector('#ytc-sug .sg a').getBoundingClientRect(); const h = document.elementFromPoint(r.left + 40, r.top + r.height / 2); return !!h && !!h.closest('#ytc-sug .sg'); }), 'a row takes clicks on the opening frame');
      await shotAt('motion-search-focus', { x: 300, y: 0, width: 800, height: 380 }, [[100, 1], [500, 2], [1500, 3]]);
      // First keystroke: in the field on its keydown frame; the text box never moves; nothing animates the input.
      await page.keyboard.type(' ');
      const k = await page.evaluate(() => ({ kd: window.__kd, in: window.__in, val: window.__val, sug: document.getElementById('ytc-sug').className, qa: document.getElementById('ytc-q').getAnimations().length }));
      assert.ok(k.kd !== undefined && k.in === k.kd, `input painted on the keydown frame (${k.kd} → ${k.in})`);
      assert.ok(k.val.endsWith(' '), k.val);
      assert.deepEqual(await box(), q0, 'the text box did not move');
      assert.equal(k.qa, 0, 'nothing animates the input');
      assert.equal(k.sug, 'sug on now', 'typing changes the dropdown with no motion');
      assert.deepEqual((await running('#ytc-sug')).filter(x => !x.startsWith('background')), [], 'typing stopped the unfold');
      await slowAnims(1);
      await page.waitForTimeout(500);
      // Settled: the pill spans the whole form, the button flush at its right end, the dropdown under the field.
      const g = await page.evaluate(() => { const f = document.getElementById('ytc-search').getBoundingClientRect(), b = document.querySelector('#ytc-search .qs').getBoundingClientRect(), u = document.getElementById('ytc-sug').getBoundingClientRect(); return { clip: getComputedStyle(document.querySelector('#ytc-search .qf'), '::before').clipPath, br: b.right - f.right, ul: u.left - f.left, ur: b.left - u.right }; });
      assert.ok(/^inset\(0px( 0px)* round 21px\)$/.test(g.clip) && g.br === 0 && g.ul === 0 && g.ur === 0, JSON.stringify(g));
      // × empties the field, keeps it focused, recent searches come back; the × spins out.
      await page.click('#ytc-search .qx');
      assert.equal(await page.inputValue('#ytc-q'), '');
      assert.equal(await page.evaluate(() => document.activeElement.id), 'ytc-q');
      await page.waitForSelector('#ytc-sug li.hs');
      await page.waitForFunction(() => getComputedStyle(document.querySelector('#ytc-search .qx')).visibility === 'hidden', null, { timeout: 1000 });
      // Pointer blur folds the field back (eased); keyboard `/` focus is instant.
      await slowAnims(0.05);
      await page.mouse.click(5, 700);
      assert.equal(await page.getAttribute('#ytc-search', 'class'), '');
      assert.ok((await running('#ytc-search')).includes('clip-path::before'), 'blur folds the field back');
      await slowAnims(1);
      await page.waitForTimeout(500);
      await slowAnims(0.05);
      await page.keyboard.press('/');
      assert.equal(await page.evaluate(() => document.activeElement.id), 'ytc-q');
      assert.equal(await page.getAttribute('#ytc-search', 'class'), 'now');
      assert.deepEqual(await running('#ytc-search'), [], 'keyboard focus: no motion');
      await page.keyboard.type('rob');
      await page.keyboard.press('Enter');
      await page.waitForFunction(() => /rob/.test(location.search));
      assert.deepEqual(await running('#ytc-search'), [], 'Enter: no motion');
      assert.equal(await page.locator('#ytc-sug li').count(), 0);
    } finally { await slowAnims(1).catch(() => {}); }
  });

  await step('motion: the sidebar pill slides between pages on pointer navigation (both edges on the spring, the leading one first), and moves at once for g+letter and back', async () => {
    HOME.rich = true;
    const pill = () => page.evaluate(() => { const e = document.getElementById('ytc-sdi'), cs = getComputedStyle(e); return { st: cs.getPropertyValue('--st'), sb: cs.getPropertyValue('--sb'), op: cs.opacity, dir: document.getElementById('ytc-side').dataset.dir || '', anims: e.getAnimations().map(a => a.transitionProperty).sort() }; });
    try {
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c');
      let p = await pill();
      assert.deepEqual([p.st, p.sb, p.op, p.anims], ['0px', '220px', '1', []], 'first paint: on Home, no motion');
      await slowAnims(0.05);
      await page.click('#ytc-side a[data-nav=history]');
      p = await pill();
      assert.deepEqual([p.dir, p.anims], ['d', ['--sb', '--st']]);
      await page.waitForTimeout(1500);
      p = await pill();
      const [st, sb] = [parseFloat(p.st), parseFloat(p.sb)];
      assert.ok(st > 0 && st < 44 && sb > 176 && sb < 220 && 220 - sb > st, `mid-slide, the leading edge ahead: top ${st}, bottom ${sb}`);
      await shotAt('motion-sidebar-slide', { x: 0, y: 56, width: 260, height: 320 }, [[0, 1], [1500, 2], [3000, 3]]);
      await slowAnims(1);
      await page.waitForFunction(() => !document.getElementById('ytc-sdi').getAnimations().length, null, { timeout: 2000 });
      const r = await page.evaluate(() => { const el = document.getElementById('ytc-sdi'), e = el.getBoundingClientRect(), a = document.querySelector('#ytc-side a[data-nav=history]').getBoundingClientRect(), cs = getComputedStyle(el); return { top: e.top + parseFloat(cs.getPropertyValue('--st')) - a.top, bottom: e.bottom - parseFloat(cs.getPropertyValue('--sb')) - a.bottom, left: e.left - a.left, right: e.right - a.right }; });
      assert.deepEqual(r, { top: 0, bottom: 0, left: 0, right: 0 }, 'settled exactly on the History link');
      assert.equal(await page.$eval('#ytc-side a[data-nav=history]', a => getComputedStyle(a).color), 'rgb(183, 235, 202)');
      // Keyboard g+w, then back: instant (no transition even with animations slowed 20×).
      await slowAnims(0.05);
      await page.keyboard.press('g'); await page.keyboard.press('w');
      await page.waitForFunction(() => document.getElementById('ytc').dataset.nav === 'wl');
      p = await pill();
      assert.deepEqual([p.st, p.sb, p.anims], ['88px', '132px', []], 'g+w: instant');
      await page.evaluate(() => history.back());
      await page.waitForFunction(() => document.getElementById('ytc').dataset.nav === 'history');
      p = await pill();
      assert.deepEqual([p.st, p.anims], ['44px', []], 'back: instant');
      // A page without a link: the pill goes where it is (Enter: at once); the next pointer navigation slides from there.
      await slowAnims(1);
      await page.fill('#ytc-q', 'pill'); await page.press('#ytc-q', 'Enter');
      await page.waitForFunction(() => document.getElementById('ytc').dataset.nav === 'search');
      p = await pill();
      assert.deepEqual([p.st, p.op, p.anims], ['44px', '0', []], 'Enter: the pill leaves at once, where it was');
      await page.click('#ytc-side a[data-nav=playlists]');
      p = await pill();
      assert.deepEqual([p.dir, p.anims], ['d', ['--sb', '--st', 'opacity']]);
    } finally { HOME.rich = false; await slowAnims(1).catch(() => {}); }
  });

  await step('motion: the bell badge grows in, pulses on a new count and the bell rings once when it rises (not on first paint); it shrinks away as the list opens', async () => {
    const tb = SIGNED_TOPBAR.desktopTopbarRenderer.topbarButtons[0].notificationTopbarButtonRenderer;
    const bell = () => page.evaluate(() => ({ n: document.getElementById('ytc-nt-n').className, text: document.getElementById('ytc-nt-n').textContent, bell: document.getElementById('ytc-nt-b').className, label: document.getElementById('ytc-nt-b').getAttribute('aria-label'),
      anims: [...document.getElementById('ytc-nt-n').getAnimations(), ...document.querySelector('#ytc-nt-b svg').getAnimations()].map(a => a.animationName || a.transitionProperty).sort() }));
    HOME.rich = true;
    try {
      tb.notificationCount = 3;
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-nt-n:not(.z)');
      let b = await bell();
      assert.deepEqual([b.n, b.text, b.bell, b.anims], ['yac-n', '3', 'yac-b', []], 'first paint: still');
      tb.notificationCount = 5;
      await slowAnims(0.05);
      await page.click('#ytc-side a[data-nav=wl]');
      await page.waitForFunction(() => document.getElementById('ytc-nt-n').textContent === '5');
      b = await bell();
      assert.ok(/^yac-n p[12]$/.test(b.n) && /^yac-b r[12]$/.test(b.bell) && b.label === 'Notifications, 5 new', JSON.stringify(b));
      assert.ok(b.anims.some(x => /^ytcpulse/.test(x)) && b.anims.some(x => /^ytcring/.test(x)), 'pulse + ring: ' + b.anims);
      await shotAt('motion-bell-ring', { x: 1200, y: 0, width: 200, height: 64 }, [[200, 1], [2400, 2], [5000, 3]]);
      await slowAnims(1);
      tb.notificationCount = 0; // opening the list marks them seen on YouTube's side too
      await page.click('#ytc-nt-b');
      b = await bell();
      assert.deepEqual([b.n, b.text, b.label], ['yac-n z', '5', 'Notifications'], 'opening shrinks the badge away');
      await page.keyboard.press('Escape');
      tb.notificationCount = 2;
      await slowAnims(0.05);
      await page.click('#ytc-side a[data-nav=liked]');
      await page.waitForFunction(() => document.getElementById('ytc-nt-n').textContent === '2');
      b = await bell();
      const op = await page.$eval('#ytc-nt-n', e => [getComputedStyle(e).opacity, getComputedStyle(e).transform]);
      // Growing = the transform is mid-way from its small start (the faster opacity fade may already be done).
      const sc = /^matrix\(([\d.]+)/.exec(op[1]);
      assert.ok(b.n === 'yac-n' && /^yac-b r[12]$/.test(b.bell) && b.anims.includes('transform') && sc && +sc[1] < 1, 'grows back in: ' + JSON.stringify(b) + JSON.stringify(op));
    } finally { tb.notificationCount = 3; HOME.rich = false; await slowAnims(1).catch(() => {}); }
  });

  await step('shots: watch actions mid-motion: Copy → Copied, a checkbox drawing, the status message unfolding (YTC_SHOTS only)', async () => {
    if (!process.env.YTC_SHOTS) return;
    try {
      await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-share-b');
      await page.click('#ytc-share-b'); await page.waitForTimeout(500);
      const b = await page.locator('#ytc-acts').boundingBox(), clip = { x: b.x + b.width - 520, y: b.y - 10, width: 540, height: 170 };
      await slowAnims(0.25); // 4× slower (the revert timer runs in real time, at 1.8 s)
      await page.click('#ytc-copy');
      await shotAt('motion-copy', clip, [[120, 1], [250, 2], [600, 3]]);
      await slowAnims(1);
      await page.mouse.click(5, 400); await page.waitForTimeout(300);
      await page.click('#ytc-save-b'); await page.waitForSelector('#ytc-save-p .ya-opt'); await page.waitForTimeout(500);
      await slowAnims(0.05);
      await page.click('#ytc-pl-1');
      await shotAt('motion-check', { x: b.x + b.width - 320, y: b.y + 30, width: 330, height: 160 }, [[600, 1], [2400, 2], [5000, 3]]);
      await slowAnims(1);
      await page.mouse.click(5, 400); await page.waitForTimeout(300);
      ACTS.fail.add('like/like');
      await slowAnims(0.05);
      await page.click('#ytc-like');
      await page.waitForFunction(() => document.getElementById('ytc-act-msg').textContent);
      await shotAt('motion-msg', clip, [[100, 1], [1500, 2], [4000, 3]]);
    } finally { ACTS.fail.clear(); await slowAnims(1).catch(() => {}); }
  });

  await step('shots: header, sidebar and popovers at 1400 and 2560 (YTC_SHOTS only)', async () => {
    if (!process.env.YTC_SHOTS) return;
    const dir = process.env.YTC_SHOTS;
    HOME.rich = true;
    try {
      for (const [w, h] of [[1400, 900], [2560, 1440]]) {
        await page.setViewportSize({ width: w, height: h });
        await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
        await page.waitForSelector('#ytc-list .c');
        await page.mouse.move(w / 2, h - 5);
        await page.waitForTimeout(400);
        await page.screenshot({ path: path.join(dir, `chrome-${w}-rest.png`), clip: { x: 0, y: 0, width: w, height: 420 } });
        await page.hover('#ytc-side a[data-nav=history]'); await page.waitForTimeout(500);
        await page.screenshot({ path: path.join(dir, `chrome-${w}-side-hover.png`), clip: { x: 0, y: 56, width: 260, height: 320 } });
        await page.click('#ytc-q'); await page.waitForTimeout(600);
        await page.screenshot({ path: path.join(dir, `chrome-${w}-search-focus.png`), clip: { x: 0, y: 0, width: w, height: 420 } });
        await page.mouse.click(5, h - 5); await page.waitForTimeout(300);
        await page.click('#ytc-nt-b'); await page.waitForSelector('#ytc-nt-p .yac-i'); await page.waitForTimeout(600);
        await page.screenshot({ path: path.join(dir, `chrome-${w}-bell.png`), clip: { x: w - 560, y: 0, width: 560, height: 420 } });
        await page.click('#ytc-ac-b'); await page.waitForFunction(() => document.querySelector('#ytc-ac-p')?.textContent.includes('E2E Viewer')); await page.waitForTimeout(600);
        await page.screenshot({ path: path.join(dir, `chrome-${w}-account.png`), clip: { x: w - 560, y: 0, width: 560, height: 420 } });
        await page.mouse.click(5, h - 5); await page.waitForTimeout(300);
      }
    } finally { HOME.rich = false; await page.setViewportSize({ width: 1400, height: 900 }); }
  });
  ACTS.signedIn = false;
  await ctx.clearCookies();

  // ---- Home timing table (simulated 150 ms InnerTube latency for Home + continuations)
  if (!ONLY) {
    HOME.rich = true; HOME.latency = 150;
    const rows = [];
    for (let i = 0; i < 5; i++) {
      await page.goto('https://www.youtube.com/results?search_query=reset', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c');
      await page.evaluate(() => sessionStorage.clear());
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length >= 30, null, { timeout: 10000 });
      const r = await page.evaluate(() => { const d = document.documentElement.dataset; return { script: +d.ytcScript, wasm: +d.ytcWasm, shell: +d.ytcShell, first: +d.ytcReady }; });
      await page.waitForTimeout(1500); // the user looks at the first screen
      r.preload = await page.evaluate(() => +document.documentElement.dataset.ytcPreloaded);
      const t0 = Date.now();
      await page.waitForFunction(() => { scrollBy(0, 1200); return document.querySelectorAll('#ytc-list .c').length >= 100; }, null, { timeout: 15000, polling: 50 });
      r.scroll100 = Date.now() - t0;
      await page.evaluate(() => scrollTo(0, 0));
      await page.waitForTimeout(200);
      await page.reload({ waitUntil: 'commit' });
      await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length >= 30 && document.documentElement.dataset.ytcReady, null, { timeout: 10000 });
      await page.waitForFunction(() => { const d = document.documentElement.dataset; return d.ytcFresh || !d.ytcRepaint; }, null, { timeout: 10000 });
      Object.assign(r, await page.evaluate(() => { const d = document.documentElement.dataset; const fcp = performance.getEntriesByName('first-contentful-paint')[0]; return { reload: +d.ytcReady, rs: +d.ytcReady - +d.ytcScript, rp: fcp ? fcp.startTime - +d.ytcScript : NaN, fresh: +(d.ytcFresh || d.ytcReady), fs: +(d.ytcFresh || d.ytcReady) - +d.ytcScript }; }));
      rows.push(r);
    }
    HOME.rich = false; HOME.latency = 0;
    const hm = k => { const v = median(rows.map(r => r[k])); return Number.isNaN(v) ? 'n/a' : v.toFixed(0); };
    console.log(`\nHome timings (${EXT}), median of 5, ms; InnerTube latency simulated at 150 ms:`);
    console.log(`  content.js start ${hm('script')} · wasm ready ${hm('wasm')} · shell ${hm('shell')} · first results ${hm('first')} · 100 videos preloaded ${hm('preload')} · scroll to 100 cards ${hm('scroll100')} · reload: cards painted ${hm('reload')} (after content.js start ${hm('rs')}, first paint ${hm('rp')}) · reload: fresh feed ${hm('fresh')} (after content.js start ${hm('fs')})`);
  }
  // ---- speed table (targets A and B): slow signed-in-like Home (+1500 ms first page,
  // 150 ms continuations) and slow `next` (300 ms). Runs on any build (YTC_EXT=<old client> YTC_TIMING_ONLY=1).
  if (!ONLY || ONLY.test('speed table')) {
    const rafWait = (fn, arg) => page.evaluate(([src, arg]) => new Promise(res => {
      const f = new Function('return ' + src)()(); const t0 = performance.now(); let first = null;
      f.start(arg);
      const tick = () => { const now = performance.now() - t0; const r = f.check(arg); if (r === 1 && first === null) first = now; if (r === 2) return res({ first: first ?? now, done: now }); if (now > 9000) return res({ first, done: NaN }); requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
    }), [fn.toString(), arg]);
    const homeRows = [], idleRows = [], focusRows = [];
    HOME.rich = true; HOME.stamp = true; HOME.delay = 1500; HOME.latency = 150; NEXT.latency = 300;
    for (let i = 0; i < 3; i++) {
      // A: Home seen, then other pages for > the background age, then Home again.
      await page.goto('https://www.youtube.com/results?search_query=speed' + i, { waitUntil: 'commit' });
      await page.evaluate(() => { sessionStorage.clear(); sessionStorage.setItem('ytc:bgage', '4000'); });
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForFunction(() => /Fresh #/.test(document.querySelector('#ytc-list .c .t')?.textContent || ''), null, { timeout: 10000 });
      const k0 = HOME.served;
      await page.mouse.move(700, 400);
      await page.fill('#ytc-q', 'a' + i); await page.press('#ytc-q', 'Enter');
      await page.waitForSelector('#ytc-list .c[data-v]');
      for (let w = 0; w < 9; w++) { await page.mouse.move(600 + w, 500); await page.waitForTimeout(500); }
      await page.fill('#ytc-q', 'b' + i); await page.press('#ytc-q', 'Enter');
      await page.waitForSelector('#ytc-list .c[data-v]');
      for (let w = 0; w < 6; w++) { await page.mouse.move(650 + w, 500); await page.waitForTimeout(400); }
      homeRows.push(await rafWait(() => ({ start: () => document.querySelector('a.logo').click(),
        check: k0 => { if (location.pathname !== '/') return 0; const t = document.querySelector('#ytc-list .c .t')?.textContent || ''; const m = /Fresh #(\d+)/.exec(t); return m && +m[1] > k0 ? 2 : t ? 1 : 0; } }), k0));
      // B: a results page left idle for 1.5 s, then a click on a card in view (no hover/press: element.click()).
      await page.goto('https://www.youtube.com/results?search_query=idle' + i, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.waitForTimeout(1500);
      idleRows.push(await rafWait(() => ({ start: () => document.querySelectorAll('#ytc-list a.t[href^="/watch"]')[3].click(),
        check: () => location.pathname === '/watch' && document.querySelector('#ytc-list .c[data-v]') ? 2 : 0 }), null));
      // B: keyboard: focus a card below the first screen, Enter 150 ms later.
      await page.goto('https://www.youtube.com/results?search_query=focus' + i, { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.evaluate(() => { const as = document.querySelectorAll('#ytc-list a.t[href^="/watch"]'), a = as[Math.min(12, as.length - 1)]; a.focus({ preventScroll: true }); });
      await page.waitForTimeout(150);
      focusRows.push(await rafWait(() => ({ start: () => document.activeElement.click(), check: () => location.pathname === '/watch' && document.querySelector('#ytc-list .c[data-v]') ? 2 : 0 }), null));
    }
    HOME.rich = false; HOME.stamp = false; HOME.delay = 0; HOME.latency = 0; NEXT.latency = 0;
    await page.evaluate(() => sessionStorage.removeItem('ytc:bgage'));
    const md = (rows, k) => { const v = median(rows.map(r => r[k])); return Number.isNaN(v) ? 'n/a' : v.toFixed(0); };
    console.log(`\nSpeed table (${EXT}), median of 3, ms; Home first page +1500 ms, next +300 ms:`);
    console.log(`  A Home click → cards visible ${md(homeRows, 'first')} · → fresh Home (newer than the last visit) ${md(homeRows, 'done')}`);
    console.log(`  B results idle 1.5 s, click a card in view → related visible ${md(idleRows, 'done')} · focus a card, Enter 150 ms later → related visible ${md(focusRows, 'done')}`);
  }
  if (TIMING_ONLY) { await ctx.close(); fs.rmSync(profile, { recursive: true, force: true }); process.exit(0); }

  // ---- timings
  if (!ONLY) {
  const cold = [], spa = [];
  for (let i = 0; i < 7; i++) {
    await page.goto(`https://www.youtube.com/results?search_query=timing${i}`, { waitUntil: 'commit' });
    await page.waitForFunction(() => document.documentElement.dataset.ytcReady);
    const t = await page.evaluate(() => {
      const nav = performance.getEntriesByType('navigation')[0];
      const fcp = performance.getEntriesByName('first-contentful-paint')[0];
      const d = document.documentElement.dataset;
      return { responseStart: nav.responseStart, script: +d.ytcScript, wasm: +d.ytcWasm, shell: +d.ytcShell, fcp: fcp ? fcp.startTime : NaN, ready: +d.ytcReady };
    });
    cold.push(t);
    // SPA navigation: click a related/result video → watch rendered.
    const t0 = await page.evaluate(() => { const a = document.querySelectorAll('#ytc-list a.t[href^="/watch"]')[3]; const t = performance.now(); a.click(); return t; });
    await page.waitForFunction(t => +document.documentElement.dataset.ytcNavReady > t, t0);
    spa.push(await page.evaluate(t => +document.documentElement.dataset.ytcNavReady - t, t0));
    await page.waitForFunction(t => +document.documentElement.dataset.ytcFirstFrame > t, t0, { timeout: 8000 });
    ff.spa.push(await page.evaluate(t => +document.documentElement.dataset.ytcFirstFrame - t, t0));
  }
  await page.keyboard.press('Backquote');
  const dbg = await page.locator('#ytc-debug').textContent();
  console.log('\nLast SPA navigation, Rust marks:\n' + dbg.split('timing')[1]);
  const m = k => median(cold.map(c => c[k]))?.toFixed(1) ?? 'n/a';
  console.log('\nTimings (ms since navigation start, median of 7, fixtures served locally by Playwright):');
  console.log(`  document response start ${m('responseStart')}  content.js start ${m('script')}  wasm ready ${m('wasm')}  shell in DOM ${m('shell')}  first contentful paint ${m('fcp')}  results rendered ${m('ready')}`);
  console.log(`  SPA click → watch page rendered (not prefetched): median ${median(spa).toFixed(1)} ms`);
  console.log(`  own player first frame (fixture media, local): direct load ${ff.direct?.toFixed(0)} ms after navigation start · SPA click → first frame median ${median(ff.spa).toFixed(0)} ms`);
  }

  // ---- features: filters, topics, Watch later, blocked websites, resume/progress
  const leveldb = () => { const dir = path.join(profile, 'Default', 'Local Extension Settings'); return fs.readdirSync(dir).flatMap(x => fs.readdirSync(path.join(dir, x)).map(f => path.join(dir, x, f))).filter(f => fs.statSync(f).isFile()).map(f => fs.readFileSync(f).toString('latin1')).join(''); };
  const cardIds = () => page.$$eval('#ytc-list .c[data-v]', cs => cs.map(c => c.dataset.v));
  const durSecs = t => t.split(':').reduce((a, x) => a * 60 + +x, 0);

  await step('filters: Search minimum length hides known short videos at once (no request), remembered across reload', async () => {
    await page.mouse.move(5, 600); // not resting on a card (hover prefetch)
    await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForSelector('#ytc-hfx .fx-fb');
    const all = await page.$$eval('#ytc-list .c', cs => cs.map(c => (c.querySelector('.th b:not(.n):not(.lv)') || {}).textContent || ''));
    // Hover opens the panel (CSS intent delay), no Live/Order/Playlists on Search.
    await page.hover('#ytc-hfx .fx-fb');
    await page.waitForFunction(() => getComputedStyle(document.querySelector('#ytc-hfx .fx-p')).visibility === 'visible');
    assert.equal(await page.locator('#ytc-hfx [data-act=fx-live], #ytc-hfx [data-act=fx-order], #ytc-hfx [data-act=fx-pl]').count(), 0);
    const n = log.api.length;
    await page.click('#ytc-hfx [data-act=fx-dur][data-arg="20"]');
    await page.waitForFunction(() => document.querySelector('#ytc-fx-dur-20')?.getAttribute('aria-pressed') === 'true');
    const kept = await page.$$eval('#ytc-list .c', cs => cs.map(c => (c.querySelector('.th b:not(.n):not(.lv)') || {}).textContent || ''));
    const short = all.filter(d => /^\d+:\d\d$/.test(d) && durSecs(d) < 1200).length;
    assert.ok(short > 0, 'fixture has short videos');
    assert.equal(kept.length, all.length - short, `kept ${kept.length} of ${all.length}`);
    assert.ok(kept.every(d => !/^\d+(:\d\d){1,2}$/.test(d) || durSecs(d) >= 1200), kept.join());
    // A display filter never re-queries YouTube (filling the freed screen space with the next page and hover/idle prefetch are normal).
    assert.deepEqual(log.api.slice(n).filter(a => a.ep === 'search' && !a.body.continuation), [], 'no new search for a display filter');
    assert.equal((await page.locator('#ytc-fx-note').textContent()) || '', '', 'filters work silently (no hidden-count note)');
    // Slider: custom value, presets follow.
    await page.$eval('#ytc-hfx input[type=range]', el => { el.value = '35'; el.dispatchEvent(new Event('input', { bubbles: true })); });
    assert.equal(await page.locator('#ytc-fx-min').textContent(), '35+ min');
    await page.click('#ytc-hfx [data-act=fx-dur][data-arg="20"]');
    await page.waitForTimeout(600); // saved after 400 ms
    await page.reload({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-hfx .fx-fb small');
    assert.equal(await page.locator('#ytc-hfx .fx-fb small').textContent(), '20+ min');
    const after = await page.$$eval('#ytc-list .c', cs => cs.map(c => (c.querySelector('.th b:not(.n):not(.lv)') || {}).textContent || ''));
    assert.ok(after.length > 0 && after.every(d => !/^\d+(:\d\d){1,2}$/.test(d) || durSecs(d) >= 1200), 'filtered right after reload: ' + after.join());
    assert.ok(leveldb().includes('yourTubeMinimumHomeDurationV1'));
    // "Any" in the (hover-open) panel resets it.
    await page.hover('#ytc-hfx .fx-fb');
    await page.click('#ytc-hfx [data-act=fx-dur][data-arg="0"]');
    // Every video is back (more may have loaded meanwhile to fill the space the filter freed).
    await page.waitForFunction(n => document.querySelectorAll('#ytc-list .c').length >= n, all.length);
  });

  await step('search filters: YouTube\'s own groups (Duration merged), hover prefetches + opens nothing, click re-queries with YouTube\'s params, chip removes, keyboard', async () => {
    const searches = () => log.api.filter(a => a.ep === 'search' && !a.body.continuation);
    await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-hfx #ytc-fx-sf-Thisyear', { state: 'attached' });
    await page.hover('#ytc-hfx .fx-fb');
    await page.waitForFunction(() => getComputedStyle(document.querySelector('#ytc-hfx .fx-p')).visibility === 'visible');
    const groups = await page.$$eval('#ytc-hfx .fx-p > .fx-row[role=group]', rs => rs.map(r => r.getAttribute('aria-label')));
    assert.deepEqual(groups.filter(g => !/^Minimum/.test(g)).slice(0, 5), ['Type', 'Duration', 'Upload date', 'Features', 'Prioritize']);
    assert.equal(groups.indexOf(groups.find(g => /^Minimum/.test(g))), 2, 'shared minimum length sits inside the Duration section');
    assert.equal(await page.locator('#ytc-hfx [data-act=fx-dur]').count(), 5, 'one length row');
    assert.equal(await page.locator('#ytc-hfx .fx-p a.fx-ch', { hasText: /^Shorts$/ }).count(), 0, 'no Shorts type');
    assert.equal(await page.locator('#ytc-hfx .fx-row[aria-label=Features] .fx-ch').count(), 11);
    // Hover an option: prefetch after the intent delay, nothing applied.
    const n = searches().length, url = page.url();
    await page.hover('#ytc-fx-sf-Thisyear');
    for (let i = 0; i < 20 && searches().length === n; i++) await page.waitForTimeout(25);
    assert.equal(searches().length, n + 1, 'hover prefetch');
    assert.equal(searches()[n].body.params, 'EgIIBQ%3D%3D', "YouTube's params verbatim");
    assert.equal(page.url(), url, 'hover applies nothing');
    await page.click('#ytc-fx-sf-Thisyear');
    await page.waitForSelector('#ytc-filters .fx-act #ytc-fx-sx-Thisyear', { state: 'attached' });
    assert.match(page.url(), /&sp=EgIIBQ%253D%253D$/);
    assert.equal(searches().length, n + 1, 'click reused the hover prefetch');
    assert.equal(await page.locator('#ytc-fx-sf-Thisyear').getAttribute('aria-current'), 'true');
    if (process.env.YTC_SHOTS) { await page.hover('#ytc-hfx .fx-fb'); await page.waitForTimeout(300); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'search-filters.png') }); }
    assert.equal(await page.locator('#ytc-fx-sf-Channels').count(), 0, 'disabled Channels is not a link');
    assert.ok(await page.locator('#ytc-list .c').count() > 5);
    // One click on the chip removes it.
    await page.mouse.move(5, 600);
    await page.click('#ytc-fx-sx-Thisyear');
    await page.waitForFunction(() => !document.querySelector('#ytc-filters .fx-act') && !location.search.includes('sp='));
    assert.equal(searches().at(-1).body.params, undefined);
    // Keyboard: Tab into the panel, Enter re-queries, focus returns to the same option.
    await page.mouse.move(5, 600);
    await page.focus('#ytc-hfx .fx-fb');
    for (let i = 0; i < 30; i++) { await page.keyboard.press('Tab'); if (await page.evaluate(() => document.activeElement?.id) === 'ytc-fx-sf-Thisweek') break; }
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => location.search.includes('sp=EgIIAw') && document.activeElement?.id === 'ytc-fx-sf-Thisweek');
    assert.equal(searches().at(-1).body.params, 'EgIIAw%3D%3D');
    assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('#ytc-hfx .fx-p')).visibility), 'visible', 'panel stays open for the next choice');
    await page.keyboard.press('Escape');
    await page.evaluate(() => document.activeElement?.blur());
  });

  await step('filters: Home popularity order (all loaded cards), Relevance restores; Music/Live labelled cards hidden by default; Playlists only', async () => {
    HOME.rich = true; FX.homeLabels = true;
    await page.evaluate(() => sessionStorage.clear());
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    let ids = await cardIds();
    assert.ok(!ids.includes(homeLabeled.music), 'Music-labelled video hidden (Music off by default)');
    assert.ok(!ids.includes(homeLabeled.live), 'live video hidden (Live off by default)');
    // The Home Filters switches sit on one line, inside the panel.
    const sw = await page.evaluate(() => { const p = document.querySelector('#ytc-hfx .fx-p').getBoundingClientRect(); const xs = [...document.querySelectorAll('#ytc-hfx .fx-sw')].map(e => e.getBoundingClientRect()); return { n: xs.length, rows: new Set(xs.map(r => Math.round(r.top))).size, inside: xs.every(r => r.left >= p.left && r.right <= p.right) }; });
    assert.ok(sw.n >= 4 && sw.rows === 1 && sw.inside, 'switches on one line: ' + JSON.stringify(sw));
    if (process.env.YTC_SHOTS) { await page.hover('#ytc-hfx .fx-fb'); await page.waitForTimeout(300); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'home-filters.png') }); await page.mouse.move(5, 600); }
    const native = ids.slice(0, 8);
    const n = log.api.length;
    await page.hover('#ytc-hfx .fx-fb');
    await page.click('#ytc-hfx [data-act=fx-order][data-arg=popular]');
    await page.waitForFunction(() => document.querySelector('#ytc-hfx [data-act=fx-order][data-arg=popular]')?.getAttribute('aria-pressed') === 'true');
    const views = await page.$$eval('#ytc-list .c .m', ms => ms.map(m => m.textContent).filter(t => /views/.test(t)).map(t => { const m = t.match(/([\d.,]+)\s*([KMB]?) views/); return m ? parseFloat(m[1].replace(/,/g, '')) * ({ K: 1e3, M: 1e6, B: 1e9 }[m[2]] || 1) : -1; }));
    assert.ok(views.length > 10);
    for (let i = 1; i < views.length; i++) assert.ok(views[i] <= views[i - 1], `sorted by views at ${i}: ${views.slice(0, i + 1)}`);
    // A re-sort brings other cards into view, and cards in view get their speculative
    // `next`/VISIONOS prefetch in idle time (by design); the order itself fetches no feed.
    assert.deepEqual(log.api.slice(n).filter(a => a.ep === 'browse' || a.ep === 'search').map(a => a.ep + ' ' + (a.body?.continuation || a.body?.browseId || '')), [], 'no feed request for ordering');
    await page.click('#ytc-hfx [data-act=fx-order][data-arg=recommended]');
    await page.waitForFunction(first => document.querySelector('#ytc-list .c')?.dataset.v === first, native[0]);
    assert.deepEqual((await cardIds()).slice(0, 8), native, 'Relevance restores the native order');
    // Music on → the labelled video returns.
    await page.click('#ytc-hfx [data-act=fx-music]');
    await page.waitForFunction(id => !!document.querySelector(`#ytc-list .c[data-v="${id}"]`), homeLabeled.music);
    await page.click('#ytc-hfx [data-act=fx-music]');
    // Playlists only: no playlists loaded → empty with a Show all reset.
    await page.click('#ytc-hfx [data-act=fx-pl]');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length === 0);
    assert.match(await page.locator('#ytc-fx-note').textContent(), /No videos match your filters/);
    await page.mouse.move(5, 600);
    await page.click('#ytc-fx-note [data-act=fx-pl]');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length > 20);
    FX.homeLabels = false;
    await page.mouse.move(5, 600);
  });

  await step('filters: Shorts switch (off by default) shows/hides Home Shorts at once, no request; Shorts badge, normal watch link, kept at any minimum length; remembered', async () => {
    HOME.rich = true; FX.homeShorts = true;
    await page.evaluate(() => sessionStorage.clear());
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    const shortIds = () => cardIds().then(ids => ids.filter(id => homeShorts.ids.includes(id)));
    const items = await page.evaluate(() => +document.documentElement.dataset.ytcItems);
    assert.deepEqual(await shortIds(), [], 'Shorts hidden by default');
    assert.equal(await page.locator('#ytc-list .th b', { hasText: /^Shorts$/ }).count(), 0);
    await page.hover('#ytc-hfx .fx-fb');
    await page.waitForFunction(() => getComputedStyle(document.querySelector('#ytc-fx-f .fx-p')).visibility === 'visible');
    assert.equal(await page.locator('#ytc-fx-shorts').getAttribute('aria-checked'), 'false');
    const n = log.api.length;
    await page.click('#ytc-hfx [data-act=fx-shorts]');
    await page.waitForFunction(id => !!document.querySelector(`#ytc-list .c[data-v="${id}"]`), homeShorts.ids[0]);
    assert.equal(await page.locator('#ytc-fx-shorts').getAttribute('aria-checked'), 'true');
    assert.deepEqual(await shortIds(), homeShorts.ids, 'all Shorts of the shelf, in native order');
    const ids = await cardIds();
    assert.equal(ids.indexOf(homeShorts.ids[0]), 2, 'in the shelf\'s place, after the first two videos');
    const card = page.locator(`#ytc-list .c[data-v="${homeShorts.ids[0]}"]`);
    assert.equal(await card.locator('.th b').textContent(), 'Shorts', 'Shorts badge in place of the duration');
    assert.equal(await card.locator('a.t').textContent(), homeShorts.title);
    assert.match(await card.locator('.m').last().textContent(), /views$/);
    assert.equal(await card.locator('a.th').getAttribute('href'), `/watch?v=${homeShorts.ids[0]}`, 'opens our watch page');
    assert.deepEqual(log.api.slice(n).filter(a => a.ep === 'browse' || a.ep === 'search').map(a => a.ep), [], 'no feed request for the switch');
    // A minimum length hides short videos but keeps the Shorts (they were asked for).
    await page.click('#ytc-hfx [data-act=fx-dur][data-arg="60"]');
    await page.waitForFunction(() => document.querySelector('#ytc-fx-dur-60')?.getAttribute('aria-pressed') === 'true');
    assert.deepEqual(await shortIds(), homeShorts.ids, 'Shorts kept at 60+ min');
    await page.click('#ytc-hfx [data-act=fx-dur][data-arg="0"]');
    // Remembered across a reload (chrome.storage, the main extension's key).
    await page.waitForTimeout(600);
    await page.reload({ waitUntil: 'commit' });
    await page.waitForFunction(id => !!document.querySelector(`#ytc-list .c[data-v="${id}"]`), homeShorts.ids[0]);
    assert.ok(leveldb().includes('shortsInFeeds'));
    // Off again: hidden at once, and Home counts as before (hidden Shorts are not loaded cards).
    await page.hover('#ytc-hfx .fx-fb');
    await page.click('#ytc-hfx [data-act=fx-shorts]');
    await page.waitForFunction(id => !document.querySelector(`#ytc-list .c[data-v="${id}"]`), homeShorts.ids[0]);
    assert.equal(await page.locator('#ytc-fx-shorts').getAttribute('aria-checked'), 'false');
    assert.deepEqual(await shortIds(), []);
    await page.waitForTimeout(600);
    await page.evaluate(() => sessionStorage.clear());
    await page.reload({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    assert.deepEqual(await shortIds(), [], 'off is remembered');
    assert.equal(await page.evaluate(() => +document.documentElement.dataset.ytcItems), items, 'same loaded count as with Shorts off before');
    FX.homeShorts = false;
    await page.mouse.move(5, 600);
  });

  await step('topic feeds: + Topic adds a topic; first cards from one anonymous fresh search (YouTube\'s This month + Videos params) in < 1 s; All restores Home; × removes it', async () => {
    await page.waitForSelector('#ytc-filters .fx-plus');
    const homeFirstIds = (await cardIds()).slice(0, 4);
    const n = log.api.length;
    await page.hover('#ytc-filters .fx-plus');
    await page.fill('#fx-topic-form input', 'Roblox animation');
    const t0 = Date.now();
    await page.press('#fx-topic-form input', 'Enter');
    await page.waitForFunction(() => document.querySelector('#ytc-filters [data-act=fx-topic][aria-pressed=true]')?.textContent === 'Roblox animation');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-list .c').length >= 3, null, { timeout: 8000 });
    const firstMs = Date.now() - t0;
    console.log(`     topic: first cards ${firstMs} ms after Enter (mocked InnerTube)`);
    assert.ok(firstMs < 1000, `first topic cards in ${firstMs} ms`);
    assert.equal(await page.locator('#ytc-fx-note').textContent(), '', 'no "Finding…" loading text');
    const reqs = log.api.slice(n);
    const searches = reqs.filter(a => a.ep === 'search');
    const before = leveldb();
    assert.equal(searches[0].body.query, 'Roblox animation tutorial');
    assert.equal(searches[0].body.params, 'EgQIBBAB', 'This month + Videos first');
    assert.equal(searches[0].body.context.client.hl, 'en');
    assert.equal(searches[0].headers.authorization, undefined);
    assert.equal(searches[0].headers.cookie, undefined, 'anonymous (credentials omitted)');
    assert.equal(reqs.filter(a => a.ep === 'next' && a.url.includes('fields=')).length, 0, 'no seed → related chain');
    assert.ok(reqs.filter(a => ['search', 'next'].includes(a.ep)).length <= 30, 'request bound');
    const shown = await cardIds();
    const titles = await page.locator('#ytc-list .c .t').allTextContents();
    assert.ok(titles.every(t => /roblox|moon animator|eclipse animator/i.test(t)), titles.join(' | '));
    // Stored as {version:1, topics:[…]}; the cards are not.
    await page.waitForTimeout(100);
    assert.ok(leveldb().includes('yourTubeRecommendationTopicsV1'));
    // (Fixture ids overlap with videos played earlier, whose resume records are stored.)
    const fresh = shown.find(id => !before.includes(id));
    assert.ok(!fresh || !leveldb().includes(fresh), 'topic cards not stored');
    // All → the native Home feed again, without a new Home request.
    const b = apiCount('browse');
    await page.click('#ytc-filters [data-act=fx-topic][data-arg="-1"]');
    await page.waitForFunction(first => document.querySelector('#ytc-list .c')?.dataset.v === first, homeFirstIds[0]);
    assert.equal(apiCount('browse'), b);
    // The × on the chip removes the topic (persisted).
    await page.hover('#ytc-filters [data-act=fx-topic][data-arg="0"]');
    await page.click('#ytc-fx-tx-0');
    await page.waitForFunction(() => !document.querySelector('#ytc-filters [data-act=fx-topic][data-arg="0"]'));
    await page.waitForTimeout(100);
    assert.deepEqual((await sw.evaluate(() => chrome.storage.local.get('yourTubeRecommendationTopicsV1'))).yourTubeRecommendationTopicsV1.topics, []);
    await page.mouse.move(5, 600);
    HOME.rich = false;
  });

  await step('no sideways scrolling: nothing is wider than the window (signed-in Home with chips, popovers closed)', async () => {
    const had = (await ctx.cookies('https://www.youtube.com')).find(c => c.name === 'SAPISID'), wasIn = ACTS.signedIn;
    await ctx.addCookies([{ name: 'SAPISID', value: 'e2e-sapisid', domain: '.youtube.com', path: '/', secure: true }]);
    ACTS.signedIn = true;
    try {
    HOME.rich = true; FX.homeChips = true;
    await page.evaluate(() => sessionStorage.clear());
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
    await page.waitForTimeout(300);
    // Classic scrollbars take layout width (macOS "Show scroll bars: Always"); headless hides them.
    await page.addStyleTag({ content: '::-webkit-scrollbar{width:15px;height:15px;background:#222}' });
    for (const width of [1400, 900]) {
      await page.setViewportSize({ width, height: 900 });
      await page.waitForTimeout(100);
      const r = await page.evaluate(() => {
        const vw = document.documentElement.clientWidth, sw = document.scrollingElement.scrollWidth;
        const wide = [...document.querySelectorAll('body *')].filter(e => { const b = e.getBoundingClientRect(); return b.width && b.right > vw + 0.5 && !e.closest('.fx-tc'); })
          .slice(0, 6).map(e => `${e.tagName.toLowerCase()}#${e.id}.${String(e.className).replace(/ /g, '.')} right=${Math.round(e.getBoundingClientRect().right)}`);
        scrollTo(500, scrollY);
        return { vw, iw: innerWidth, sw, x: scrollX, wide, acct: document.querySelectorAll('#ytc-acct button').length };
      });
      assert.ok(r.sw <= r.vw && r.x === 0, `page scrolls sideways at ${width}px: ` + JSON.stringify(r));
    }
    // Sidebar rhythm: its first row sits on the chip row (same top, same height); the logo's tile is centred over the icons.
    const al = await page.evaluate(() => { const r = e => e.getBoundingClientRect(); const side = r(document.querySelector('#ytc-side a')), chip = r(document.querySelector('#ytc-filters .fx-tc > .fx-ch')), logo = r(document.querySelector('#ytc .logo svg')), icon = r(document.querySelector('#ytc-side a svg'));
      return { top: +(side.top - chip.top).toFixed(1), h: +(side.height - chip.height).toFixed(1), logoX: +((logo.left + logo.width / 2) - (icon.left + icon.width / 2)).toFixed(1) }; });
    assert.deepEqual(al, { top: 0, h: 0, logoX: 0 }, 'sidebar aligned with the chips and the logo: ' + JSON.stringify(al));
    // Every icon-only button: its icon sits on the button's centre (±1 px), e.g. the search magnifier.
    const off = await page.evaluate(() => [...document.querySelectorAll('button')].filter(b => b.offsetParent && b.children.length === 1 && b.firstElementChild.tagName === 'svg' && !b.textContent.trim())
      .map(b => { const a = b.getBoundingClientRect(), i = b.firstElementChild.getBoundingClientRect(); return { id: b.id || b.className || b.getAttribute('aria-label'), dx: +((i.left + i.width / 2) - (a.left + a.width / 2)).toFixed(1), dy: +((i.top + i.height / 2) - (a.top + a.height / 2)).toFixed(1) }; })
      .filter(o => Math.abs(o.dx) > 1 || Math.abs(o.dy) > 1));
    assert.deepEqual(off, [], 'icons centred in their buttons');
    // The card's Watch later bookmark (and its round hover wash) sits inside the card, which clips its paint.
    const wl = await page.evaluate(() => { const c = document.querySelector('#ytc-list .c:has(.wl)'); if (!c) return null; const a = c.getBoundingClientRect(), b = c.querySelector('.wl').getBoundingClientRect(); return { out: Math.max(0, b.right - a.right, a.left - b.left, a.top - b.top) }; });
    assert.ok(wl && wl.out < 0.5, 'bookmark inside its card: ' + JSON.stringify(wl));
    } finally {
      await page.setViewportSize({ width: 1400, height: 900 });
      ACTS.signedIn = wasIn;
      if (had) await ctx.addCookies([had]); else await ctx.clearCookies({ name: 'SAPISID' });
    }
  });

  await step('YouTube Home chips: beside the own topics, switch by the chip\'s token, All restores without a request, reorder/hide remembered', async () => {
    HOME.rich = true; FX.homeChips = true;
    await page.evaluate(() => sessionStorage.clear());
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
    const bar = () => page.$$eval('#ytc-filters .fx-tc > .fx-ch', bs => bs.map(b => b.textContent));
    assert.deepEqual((await bar()).slice(0, 3), ['All', 'Gaming', 'Podcasts'], 'YouTube chips right after All, no duplicate All');
    // One sideways-scrolling row (never a second line), wheel scrolls it, the page never rubber-bands.
    // (The fixture has three chips: the row is squeezed to make it overflow.)
    await page.evaluate(() => { document.getElementById('ytc-fx-tc').style.maxWidth = '140px'; });
    const row = await page.evaluate(() => { const t = document.getElementById('ytc-fx-tc'), ed = document.getElementById('ytc-fx-ed');
      return { over: t.scrollWidth > t.clientWidth, tops: new Set([...t.children].map(c => c.getBoundingClientRect().top)).size, edTop: Math.round(ed.getBoundingClientRect().top - t.children[0].getBoundingClientRect().top), osb: getComputedStyle(document.documentElement).overscrollBehaviorY }; });
    assert.deepEqual(row, { over: true, tops: 1, edTop: 0, osb: 'none' }, 'chips in one row');
    await page.mouse.move((await page.locator('#ytc-fx-tc').boundingBox()).x + 60, (await page.locator('#ytc-fx-tc').boundingBox()).y + 20);
    await page.mouse.wheel(0, 300);
    await page.waitForFunction(() => document.getElementById('ytc-fx-tc').scrollLeft > 0);
    assert.equal(await page.evaluate(() => scrollY), 0, 'the wheel scrolled the chips, not the page');
    await page.mouse.move(5, 600);
    await page.evaluate(() => { document.getElementById('ytc-fx-tc').style.maxWidth = ''; });
    const allIds = (await cardIds()).slice(0, 4);
    const conts = () => log.api.filter(a => a.ep === 'browse' && /^CHIP/.test(a.body.continuation || '')).map(a => a.body.continuation);
    await page.click('#ytc-fx-nchip-gaming');
    await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H7z'));
    assert.deepEqual(conts(), ['CHIPGAMING'], "the chip's own continuation token");
    if (process.env.YTC_SHOTS) { await page.hover('#ytc-filters .fx-plus'); await page.waitForTimeout(300); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'home-chips.png') }); await page.mouse.move(5, 600); }
    assert.equal(await page.locator('#ytc-fx-nchip-gaming').getAttribute('aria-pressed'), 'true');
    assert.equal(await page.locator('#ytc-filters [data-act=fx-topic][data-arg="-1"]').getAttribute('aria-pressed'), 'false');
    await page.click('#ytc-fx-nchip-podcasts');
    await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H8z'));
    assert.equal(conts()[1], 'CHIPPOD2', "token from the current chip response's bar");
    const b = apiCount('browse');
    await page.click('#ytc-filters [data-act=fx-topic][data-arg="-1"]');
    await page.waitForFunction(first => document.querySelector('#ytc-list .c')?.dataset.v === first, allIds[0]);
    assert.equal(apiCount('browse'), b, 'All from memory');
    // Editor: move Podcasts earlier, hide Gaming; remembered across a reload.
    await page.hover('#ytc-filters .fx-plus');
    await page.waitForTimeout(300);
    const frames = () => page.evaluate(() => new Promise(r => { const out = []; const f = () => { const p = document.querySelector('#ytc-fx-ed .fx-p'), cs = getComputedStyle(p); out.push(cs.visibility[0] + (+cs.opacity).toFixed(2)); if (out.length < 12) requestAnimationFrame(f); else r(out.join(' ')); }; f(); }));
    // The editor re-renders on each change and must neither blink nor replay its entrance; hiding a
    // chip shifts it under the still pointer, so it stays pinned until the pointer leaves it.
    await page.click('#ytc-fx-nup-podcasts');
    assert.equal(await frames(), Array(12).fill('v1.00').join(' '), 'editor steady after ↑');
    await page.click('#ytc-fx-nhide-gaming');
    assert.equal(await frames(), Array(12).fill('v1.00').join(' '), 'editor steady after Hide');
    // (The rows reflow on each change (FLIP), so the click above waited for them to settle; pointer moves are
    // reported at most every 150 ms: leave a real pause before the one jump away.)
    await page.waitForTimeout(160);
    await page.mouse.move(5, 600);
    await page.waitForFunction(() => getComputedStyle(document.querySelector('#ytc-fx-ed .fx-p')).visibility === 'hidden', null, { timeout: 1000 });
    assert.deepEqual((await bar()).slice(0, 2), ['All', 'Podcasts']);
    await page.waitForTimeout(100);
    assert.ok(leveldb().includes('yourTubeNativeTopicsV1'));
    assert.ok(!leveldb().includes('CHIPGAMING') && !leveldb().includes('CHIPPOD'), 'chip tokens never stored');
    await page.reload({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-filters [data-act=fx-nchip]').length === 1);
    assert.deepEqual((await bar()).slice(0, 2), ['All', 'Podcasts']);
    // Keyboard: Show Gaming again from the editor; focus stays on the control.
    await page.focus('#ytc-fx-ed-btn');
    for (let i = 0; i < 20; i++) { await page.keyboard.press('Tab'); if (await page.evaluate(() => document.activeElement?.id) === 'ytc-fx-nhide-gaming') break; }
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => document.querySelectorAll('#ytc-filters [data-act=fx-nchip]').length === 2 && document.activeElement?.id === 'ytc-fx-nhide-gaming');
    assert.deepEqual((await bar()).slice(0, 3), ['All', 'Podcasts', 'Gaming']);
    await page.evaluate(() => document.activeElement?.blur());
    // Rename (inline field in the editor): Enter saves and the chip shows the name at once (no request),
    // Escape cancels, empty restores YouTube's label; the name is stored, the feed still the chip's own.
    const ren = '#ytc-fx-ed li[data-k="gaming"] input';
    const nb = apiCount('browse');
    await page.hover('#ytc-filters .fx-plus');
    await page.waitForTimeout(300);
    await page.click(ren);
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('My games');
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => document.getElementById('ytc-fx-nchip-gaming')?.textContent === 'My games');
    assert.deepEqual((await bar()).slice(0, 3), ['All', 'Podcasts', 'My games']);
    assert.equal(await page.inputValue(ren), 'My games');
    assert.equal(await page.evaluate(() => document.activeElement?.closest('li')?.dataset.k), 'gaming', 'focus stays on the row');
    await shot('chip-rename');
    const stored = async () => (await sw.evaluate(() => chrome.storage.local.get('yourTubeNativeTopicsV1'))).yourTubeNativeTopicsV1.topics;
    assert.ok((await stored()).some(t => t.key === 'gaming' && t.name === 'My games'), 'name stored');
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('Junk');
    await page.keyboard.press('Escape');
    assert.equal(await page.inputValue(ren), 'My games', 'Escape cancels');
    assert.equal(await page.locator('#ytc-fx-nchip-gaming').textContent(), 'My games');
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => document.getElementById('ytc-fx-nchip-gaming')?.textContent === 'Gaming');
    assert.ok((await stored()).every(t => t.name !== 'My games'), 'empty restores the original');
    assert.equal(apiCount('browse'), nb, 'renaming asks nothing');
    await page.evaluate(() => document.activeElement?.blur());
    await page.mouse.move(5, 600);
    await sw.evaluate(() => chrome.storage.local.remove('yourTubeNativeTopicsV1'));
    HOME.rich = false; FX.homeChips = false;
  });

  await step('feed motion: chip pill slides from the old chip to the new one (pointer; keyboard instant), Filters segment pill, entrance wave only on fresh pointer loads (never back/forward/restore), a card clicks mid-wave', async () => {
    const slow = rate => cdp.send('Animation.setPlaybackRate', { playbackRate: rate });
    await cdp.send('Animation.enable');
    HOME.rich = true; FX.homeChips = true;
    const box = sel => page.$eval(sel, e => { const r = e.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; });
    // The pill copy's clip as a viewport box [x, y, w, h].
    const pill = () => page.evaluate(() => {
      const g = document.querySelector('#ytc-fx-tc > .fx-pg'); if (!g) return null;
      const m = getComputedStyle(g).clipPath.match(/inset\(([-\d.]+)px ([-\d.]+)px ([-\d.]+)px ([-\d.]+)px/); if (!m) return getComputedStyle(g).clipPath;
      const r = g.getBoundingClientRect(), [t, ri, b, l] = m.slice(1).map(Number);
      return [r.left + l, r.top + t, r.width - l - ri, r.height - t - b];
    });
    try {
      await page.evaluate(() => sessionStorage.clear());
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
      await page.waitForSelector('#ytc-list .c');
      await page.mouse.move(5, 600);
      const all = await box('#ytc-fx-topic--1'), gaming = await box('#ytc-fx-nchip-gaming'), pod = await box('#ytc-fx-nchip-podcasts');
      // Pointer: the pill starts on All and slides (and widens) to Gaming; the copy lies exactly on the row.
      await slow(0.02);
      await page.click('#ytc-fx-nchip-gaming');
      const p0 = await pill();
      assert.ok(Array.isArray(p0), 'pill copy present: ' + p0);
      assert.ok(Math.abs(p0[0] - all[0]) < Math.abs(p0[0] - gaming[0]) && p0[0] < gaming[0] - 2, `pill starts at All (${p0} vs All ${all}, Gaming ${gaming})`);
      const fit = await page.evaluate(() => { const a = document.querySelector('#ytc-fx-tc > .fx-pg [data-of=ytc-fx-nchip-gaming]').getBoundingClientRect(), b = document.getElementById('ytc-fx-nchip-gaming').getBoundingClientRect(); return Math.max(...['left', 'top', 'width', 'height'].map(k => Math.abs(a[k] - b[k]))); });
      assert.ok(fit < 0.5, `the copy covers the real chips to the pixel (off by ${fit})`);
      const st = await page.evaluate(() => { const r = document.getElementById('ytc-fx-nchip-gaming').getBoundingClientRect(), h = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return { pill: document.getElementById('ytc-fx-tc').dataset.pill, pressed: document.getElementById('ytc-fx-nchip-gaming').getAttribute('aria-pressed'), hit: h && h.id }; });
      assert.deepEqual(st, { pill: '1', pressed: 'true', hit: 'ytc-fx-nchip-gaming' }, 'selection final at once; the real chip takes the clicks');
      if (process.env.YTC_SHOTS) { await slow(0.1); await page.waitForTimeout(700); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'motion-chip-pill-mid.png'), clip: { x: Math.max(0, all[0] - 20), y: all[1] - 16, width: 520, height: all[3] + 32 } }); await slow(0.02); }
      const p1 = await pill();
      // The feed answers mid-slide (the row is re-rendered): the slide carries on, it never restarts or jumps.
      await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H7z'));
      const p2 = await pill();
      assert.ok(Array.isArray(p2) && p2[0] >= p1[0] - 0.5 && p2[0] <= gaming[0] + 0.5, `carried over the re-rendered row (${p1} → ${p2})`);
      await slow(1);
      await page.waitForFunction(() => !document.querySelector('#ytc-fx-tc > .fx-pg') && !document.getElementById('ytc-fx-tc').dataset.pill, null, { timeout: 2000 });
      assert.equal(await page.$eval('#ytc-fx-nchip-gaming', b => getComputedStyle(b).backgroundColor), 'rgb(183, 235, 202)', 'the real chip took over');
      // Keyboard: instant (no copy, no pill state).
      await page.focus('#ytc-fx-nchip-podcasts');
      await slow(0.02);
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => [!!document.querySelector('#ytc-fx-tc > .fx-pg'), document.getElementById('ytc-fx-tc').dataset.pill || '', document.getElementById('ytc-fx-nchip-podcasts').getAttribute('aria-pressed'), getComputedStyle(document.getElementById('ytc-fx-nchip-podcasts')).backgroundColor]),
        [false, '', 'true', 'rgb(183, 235, 202)'], 'keyboard selection instant');
      await slow(1);
      await page.evaluate(() => document.activeElement?.blur());
      await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H8z'));
      // A chip's first cards enter as a wave (pointer), and the All feed from memory paints at once.
      await page.mouse.move(5, 600);
      const listAnims = () => page.evaluate(() => document.getAnimations().filter(a => a.effect?.target?.closest?.('#ytc-list') && a.playState === 'running').length);
      await page.click('#ytc-fx-topic--1');
      assert.equal(await listAnims(), 0, 'All (memory) paints at once');
      await slow(0.05);
      await page.click('#ytc-fx-nchip-gaming');
      await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H7z'));
      const wave = await listAnims();
      assert.ok(wave >= 5 && wave <= 15, `chip feed enters as a capped wave (${wave} cards)`);
      await slow(1);
      await page.click('#ytc-fx-topic--1');
      await page.waitForTimeout(200);

      // Filters segments: the pill slides between Any → 20+ (pointer), keyboard instant.
      await page.hover('#ytc-hfx .fx-fb');
      await page.waitForTimeout(400);
      await slow(0.02);
      await page.click('#ytc-fx-dur-20');
      const seg = await page.evaluate(() => { const row = document.getElementById('ytc-fx-durrow'), g = row.querySelector(':scope > .fx-pg'); return { pill: row.dataset.pill, copy: !!g, n: g ? g.children.length : 0 }; });
      assert.deepEqual(seg, { pill: '1', copy: true, n: 5 }, 'segment pill slides');
      await slow(1);
      await page.waitForFunction(() => !document.querySelector('#ytc-fx-durrow > .fx-pg'), null, { timeout: 2000 });
      await page.focus('#ytc-fx-dur-0');
      await page.keyboard.press('Enter');
      assert.equal(await page.evaluate(() => !!document.querySelector('#ytc-fx-durrow > .fx-pg')), false, 'keyboard segment instant');
      assert.equal(await page.getAttribute('#ytc-fx-dur-0', 'aria-pressed'), 'true');
      await page.evaluate(() => document.activeElement?.blur());
      await page.mouse.move(5, 600);
      await page.waitForTimeout(300);
      HOME.rich = false; FX.homeChips = false;

      // Entrance wave: a fresh pointer load that replaces a skeleton (a channel page); never on back/forward.
      const textAnims = () => page.evaluate(() => document.getAnimations().filter(a => a.effect?.target?.matches?.('#ytc-list .c > .d')).length);
      await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c .m a');
      await page.waitForTimeout(300);
      assert.equal(await textAnims(), 0, 'no wave on the first page load');
      await slow(0.05);
      await page.click('#ytc-list .c .m a[href^="/@"]');
      await page.waitForSelector('.ch h1');
      await page.waitForSelector('#ytc-list .c:not(.sk)');
      const w = await textAnims();
      assert.ok(w >= 5 && w <= 15, `fresh pointer load: the text replaces the skeleton bars in a wave (${w})`);
      if (process.env.YTC_SHOTS) { await page.waitForTimeout(1500); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'motion-entrance-wave-mid.png'), clip: { x: 0, y: 0, width: 1400, height: 760 } }); }
      // A card clicks mid-wave: the watch page opens.
      await page.click('#ytc-list .c:nth-child(2) a.t');
      await page.waitForFunction(() => location.pathname === '/watch' && document.querySelector('#ytc-info h1.wt'));
      await slow(1);
      await page.waitForTimeout(400);
      // Back (memory restore) and forward again: no wave.
      await slow(0.05);
      await page.evaluate(() => history.back());
      await page.waitForFunction(() => document.querySelector('.ch h1') && document.querySelectorAll('#ytc-list .c').length > 5);
      assert.equal(await textAnims(), 0, 'back: restored without a wave');
      await page.evaluate(() => history.back());
      await page.waitForFunction(() => location.pathname === '/results' && document.querySelectorAll('#ytc-list .c').length > 5);
      assert.equal(await textAnims(), 0, 'back again: no wave');
      await page.evaluate(() => history.forward());
      await page.waitForFunction(() => document.querySelector('.ch h1') && document.querySelectorAll('#ytc-list .c').length > 5);
      assert.equal(await textAnims(), 0, 'forward: no wave');
      await slow(1);
    } finally {
      await slow(1);
      HOME.rich = false; FX.homeChips = false;
    }
  });

  // YTC_SHOTS only: the same states at 1400 and 2560 px (works on older builds too, for before/after), then
  // mid-animation frames with the animations slowed (new motion only; skipped where a build lacks it).
  if (process.env.YTC_SHOTS) await step('shots: feeds, chips, Filters and the bookmark at 1400 and 2560 (YTC_SHOTS)', async () => {
    const dir = process.env.YTC_SHOTS, slow = rate => cdp.send('Animation.setPlaybackRate', { playbackRate: rate });
    await cdp.send('Animation.enable');
    HOME.rich = true; FX.homeChips = true; FX.wlMock = true;
    const had = (await ctx.cookies('https://www.youtube.com')).find(c => c.name === 'SAPISID');
    await ctx.addCookies([{ name: 'SAPISID', value: 'e2eSAPISID', domain: '.youtube.com', path: '/', secure: true }]);
    const shot = (name, clip) => page.screenshot({ path: path.join(dir, name + '.png'), ...(clip ? { clip } : {}) });
    try {
      for (const w of [1400, 2560]) {
        await page.setViewportSize({ width: w, height: w === 1400 ? 900 : 1440 });
        await page.evaluate(() => sessionStorage.clear());
        await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
        await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
        await page.waitForSelector('#ytc-list .c');
        await page.click('#ytc-fx-nchip-gaming');
        await page.waitForFunction(() => document.querySelector('#ytc-list .c')?.dataset.v?.includes('H7z'));
        await page.mouse.move(5, 600);
        await page.waitForTimeout(700);
        await page.hover('#ytc-list .c:nth-child(2) a.th');
        await page.waitForTimeout(800);
        await shot(`home-hover-${w}`, { x: 0, y: 0, width: w, height: w === 1400 ? 620 : 760 });
        const card = await page.locator('#ytc-list .c:nth-child(2)').boundingBox();
        await shot(`card-hover-${w}`, { x: card.x - 12, y: card.y - 12, width: card.width + 24, height: card.height + 24 });
        await page.mouse.move(5, 600);
        await page.hover('#ytc-hfx .fx-fb');
        await page.waitForTimeout(600);
        const fp = await page.locator('#ytc-hfx .fx-p').boundingBox();
        await shot(`filters-open-${w}`, { x: fp.x - 40, y: 0, width: fp.width + 80, height: fp.y + fp.height + 50 });
        await page.mouse.move(5, 600);
        await page.waitForTimeout(400);
        await page.hover('#ytc-filters .fx-plus');
        await page.waitForTimeout(600);
        const ep = await page.locator('#ytc-fx-ed .fx-p').boundingBox();
        await shot(`topic-editor-open-${w}`, { x: Math.max(0, ep.x - 260), y: ep.y - 70, width: ep.width + 300, height: ep.height + 110 });
        await page.mouse.move(5, 600);
        await page.waitForTimeout(400);
      }
      await page.setViewportSize({ width: 1400, height: 900 });
      await page.mouse.move(5, 600);
      // Mid-animation frames (20× slower).
      const has = sel => page.evaluate(sel => !!document.querySelector(sel), sel);
      const hdr = { x: 700, y: 0, width: 700, height: 560 };
      await slow(0.05);
      await page.hover('#ytc-hfx .fx-fb');
      await page.waitForTimeout(1600);
      await shot('mid-filters-morph-a', hdr);
      await page.waitForTimeout(1600);
      await shot('mid-filters-morph-b', hdr);
      await slow(1);
      await page.waitForTimeout(300);
      if (await has('#ytc-fx-durrow')) {
        await slow(0.05);
        await page.click('#ytc-fx-dur-30');
        await page.waitForTimeout(1500);
        await shot('mid-segment-pill', hdr);
        await slow(1);
        await page.waitForTimeout(500);
        await page.click('#ytc-fx-dur-0');
      }
      await page.mouse.move(5, 600);
      await page.waitForTimeout(400);
      const card = page.locator('#ytc-list .c:nth-child(3)');
      await slow(0.05);
      await card.locator('a.th').hover();
      await page.waitForTimeout(2500);
      const cb = await card.boundingBox();
      await shot('mid-card-hover-zoom', { x: cb.x - 12, y: cb.y - 12, width: cb.width + 24, height: cb.height + 24 });
      await slow(1);
      const wl = card.locator('.wl');
      if (await wl.count()) {
        await wl.hover();
        await page.waitForTimeout(400);
        await slow(0.05);
        await wl.click();
        await page.waitForTimeout(1200);
        const b = await wl.boundingBox();
        await shot('mid-bookmark-fill-pop', { x: b.x - 30, y: b.y - 20, width: b.width + 60, height: b.height + 40 });
        await slow(1);
        await page.waitForTimeout(800);
        await shot('bookmark-saved', { x: b.x - 30, y: b.y - 20, width: b.width + 60, height: b.height + 40 });
      }
      await page.mouse.move(5, 600);
      // A YouTube chip hidden from the editor: its copy shrinks out while the row closes the gap.
      await page.hover('#ytc-filters .fx-plus');
      await page.waitForTimeout(500);
      const row = await page.locator('#ytc-filters').boundingBox();
      await slow(0.05);
      await page.click('#ytc-fx-nhide-gaming', { force: true });
      await page.waitForTimeout(1200);
      await shot('mid-chip-hide-reflow', { x: row.x, y: row.y - 10, width: 640, height: 70 });
      await slow(1);
      await page.waitForTimeout(600);
      await page.click('#ytc-fx-nhide-gaming', { force: true });
      await page.mouse.move(5, 600);
      await page.waitForTimeout(400);
      await sw.evaluate(() => chrome.storage.local.remove('yourTubeNativeTopicsV1'));
    } finally {
      await slow(1);
      await page.setViewportSize({ width: 1400, height: 900 });
      HOME.rich = false; FX.homeChips = false; FX.wlMock = false;
      if (had) await ctx.addCookies([had]); else await ctx.clearCookies({ name: 'SAPISID' });
    }
  });

  await step('Watch later (signed in, mocked): lazy saved check on hover, save/remove through browse/edit_playlist, error state with reason, keyboard', async () => {
    FX.wlMock = true;
    await ctx.addCookies([{ name: 'SAPISID', value: 'e2eSAPISID', domain: '.youtube.com', path: '/', secure: true }]);
    await page.goto('https://www.youtube.com/results?search_query=roblox+studio+animation', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c .wl');
    const v = await page.$eval('#ytc-list .c .wl', b => b.dataset.arg);
    if (process.env.YTC_SHOTS) { await page.hover(`#ytc-list .c[data-v="${v}"] .t`); await page.waitForTimeout(200); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'search-wl.png'), clip: { x: 0, y: 0, width: 1400, height: 520 } }); }
    FX.wl.add(v); // already saved on the server
    const n = log.api.length;
    await page.hover(`#ytc-wl-${v}`);
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).classList.contains('saved'), v);
    const probe = log.api.slice(n).find(a => a.ep === 'playlist/get_add_to_playlist');
    assert.deepEqual(probe.body.videoIds, [v]);
    assert.match(probe.headers.authorization, /^SAPISIDHASH \d+_[0-9a-f]{40}/);
    await page.hover(`#ytc-wl-${v}`);
    assert.equal(log.api.slice(n).filter(a => a.ep === 'playlist/get_add_to_playlist').length, 1, 'asked once');
    // Remove, then add again with the keyboard.
    await page.click(`#ytc-wl-${v}`);
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).className === 'ca wl', v);
    let ed = log.api.filter(a => a.ep === 'browse/edit_playlist').at(-1);
    assert.equal(ed.body.playlistId, 'WL');
    assert.deepEqual(ed.body.actions, [{ action: 'ACTION_REMOVE_VIDEO_BY_VIDEO_ID', removedVideoId: v }]);
    await page.focus(`#ytc-wl-${v}`);
    await page.keyboard.press('Enter');
    assert.ok(await page.$eval(`#ytc-wl-${v}`, b => b.classList.contains('on') || b.classList.contains('saved')), 'the bookmark fills on the click (optimistic)');
    assert.ok(!(await page.$eval(`#ytc-wl-${v}`, b => b.classList.contains('pop'))), 'a keyboard save does not pop');
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).classList.contains('saved'), v);
    ed = log.api.filter(a => a.ep === 'browse/edit_playlist').at(-1);
    assert.deepEqual(ed.body.actions, [{ addedVideoId: v, action: 'ACTION_ADD_VIDEO' }]);
    assert.ok(ed.headers.authorization.startsWith('SAPISIDHASH '));
    assert.equal(await page.getAttribute(`#ytc-wl-${v}`, 'aria-label'), 'Saved to Watch later. Click to remove.');
    // Pointer: a remove drains the fill; a save fills it from the bottom up (clip-path) and pops once
    // (--spring-pop), the pop surviving the server's confirmation.
    await page.click(`#ytc-wl-${v}`);
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).className === 'ca wl', v);
    await page.waitForTimeout(250);
    assert.equal(await page.$eval(`#ytc-wl-${v} svg.f`, s => getComputedStyle(s).clipPath), 'inset(100% 0px 0px)', 'drained');
    await page.click(`#ytc-wl-${v}`);
    const pop = await page.$eval(`#ytc-wl-${v}`, b => ({ cls: b.className, anims: b.getAnimations({ subtree: true }).map(a => a.animationName || a.transitionProperty).sort().join(' ') }));
    assert.ok(/\bpop\b/.test(pop.cls) && /ytcwlpop/.test(pop.anims) && /clip-path/.test(pop.anims), JSON.stringify(pop));
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).classList.contains('saved'), v);
    assert.ok(await page.$eval(`#ytc-wl-${v}`, b => b.classList.contains('pop')), 'pop kept through the confirmation');
    await page.waitForTimeout(700);
    assert.deepEqual(await page.$eval(`#ytc-wl-${v}`, b => [getComputedStyle(b.querySelector('svg.f')).clipPath, getComputedStyle(b.querySelector('svg.o')).scale]), ['inset(0px)', 'none'], 'filled, at rest');
    // Failure keeps the reason and retries the same edit.
    FX.wlFail = true;
    await page.click(`#ytc-wl-${v}`);
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).classList.contains('error'), v);
    assert.equal(await page.getAttribute(`#ytc-wl-${v}`, 'aria-label'), 'Couldn’t remove from Watch later: Sign in required (401). Click to retry.');
    FX.wlFail = false;
    await page.click(`#ytc-wl-${v}`);
    await page.waitForFunction(id => document.getElementById('ytc-wl-' + id).className === 'ca wl', v);
    assert.ok(!FX.wl.has(v));
    await ctx.clearCookies();
    await page.reload({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    assert.equal(await page.locator('#ytc-list .wl').count(), 0, 'no button signed out');
    FX.wlMock = false;
  });

  await step('blocked websites: Block beside a description link; related cards whose own description links there are hidden (masked anonymous next, ≤ 2 at once); Undo', async () => {
    await page.goto('https://www.youtube.com/watch?v=blockLINK01', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-desc .fx-blk');
    assert.equal(await page.locator('#ytc-desc .fx-blk').textContent(), 'Block example.net');
    await page.waitForSelector('#ytc-list .c');
    const rel = await cardIds();
    // Cards below the first screen: the ones in view were prefetched speculatively (descriptions known).
    const linked = rel.slice(10, 13);
    FX.linked = new Set(linked);
    FX.checkDelay = 120;
    const n = log.api.length;
    await page.hover('#ytc-desc a[target=_blank]');
    await page.click('#ytc-desc .fx-blk');
    await page.waitForFunction(() => document.querySelector('#ytc-desc .fx-blk')?.textContent === 'Blocked · Undo');
    await page.waitForFunction(ids => ids.every(id => !document.querySelector(`#ytc-list .c[data-v="${id}"]`)), linked, { timeout: 10000 });
    await page.waitForFunction(n => document.querySelectorAll('#ytc-list .c').length === n, rel.length - 3, { timeout: 10000 });
    const isCheck = a => a.ep === 'next' && a.url.includes('fields=');
    // Wait for the queue to drain (cards whose next response was already in memory need no check).
    for (let i = 0, last = -1; i < 100; i++) { const c = log.api.slice(n).filter(isCheck).length; if (c === last && !FX.checksInFlight) break; last = c; await page.waitForTimeout(300); }
    const checks = log.api.slice(n).filter(isCheck);
    assert.ok(checks.length >= 3, `checks ${checks.length}`);
    assert.equal(new Set(checks.map(c => c.body.videoId)).size, checks.length, 'each video checked once');
    assert.ok(checks.every(c => !c.headers.cookie && !c.headers.authorization));
    assert.ok(FX.checksMax <= 2, `concurrent checks ${FX.checksMax}`);
    const st = leveldb();
    assert.ok(st.includes('yourTubeBlockedWebsitesV1') && st.includes('example.net'));
    assert.ok(!st.includes('Get the plugin'), 'description text never stored');
    // Undo: the rule goes, the cards come back without new checks.
    const m = log.api.length;
    await page.click('#ytc-desc .fx-blk');
    // The linked cards come back (the taller related list may have loaded a second batch meanwhile).
    await page.waitForFunction(ids => ids.every(id => document.querySelector(`#ytc-list .c[data-v="${id}"]`)), linked);
    // Only website checks count: hover/idle prefetch of cards under the pointer is normal.
    assert.equal(log.api.slice(m).filter(isCheck).length, 0, 'verdicts re-evaluated from memory');
    FX.checkDelay = 0; FX.linked = new Set();
  });

  await step('blocked websites: appended Home batches hold back unchecked cards until their check answers; blocked cards never appear', async () => {
    // Block example.net again (from the description), then open a rich Home.
    await page.goto('https://www.youtube.com/watch?v=blockLINK01', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-desc .fx-blk');
    await page.hover('#ytc-desc a[target=_blank]');
    await page.click('#ytc-desc .fx-blk');
    await page.waitForFunction(() => document.querySelector('#ytc-desc .fx-blk')?.textContent === 'Blocked · Undo');
    HOME.rich = true;
    // Every video of continuation pages 1–2 links to the blocked site.
    FX.linked = new Set([...ids11(homeConts[0]), ...ids11(homeConts[1])]);
    FX.checkDelay = 150; FX.homeMax = 0;
    // From the very first frame of the next document: was any blocked card ever displayed?
    if (!holdWatch) { holdWatch = true; await page.addInitScript(() => {
      if (!sessionStorage.getItem('ytc-e2e-hold')) return;
      const linked = JSON.parse(sessionStorage.getItem('ytc-e2e-hold')), seen = window.__hold = { flashed: [], frames: 0 };
      const f = () => { seen.frames++; for (const id of linked) { const c = document.querySelector(`#ytc-list .c[data-v="${id}"]`); if (c && c.getClientRects().length && !seen.flashed.includes(id)) seen.flashed.push(id); } requestAnimationFrame(f); };
      requestAnimationFrame(f);
    }); }
    await page.evaluate(linked => { sessionStorage.clear(); sessionStorage.setItem('ytc-e2e-hold', JSON.stringify(linked)); }, [...FX.linked]);
    await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c');
    const first = await page.$$eval('#ytc-list .c:not(.fxh)', cs => cs.length);
    await page.waitForFunction(() => +document.documentElement.dataset.ytcItems >= 100, null, { timeout: 8000 });
    // Scroll fast: two more chunks are appended right away.
    const before = await page.$$eval('#ytc-list .c', cs => cs.length);
    await page.waitForFunction(n => { scrollBy(0, 2000); return document.querySelectorAll('#ytc-list .c').length >= n + 40; }, before, { timeout: 8000, polling: 50 });
    try { await page.waitForFunction(() => !document.querySelector('#ytc-list .c.fxh'), null, { timeout: 20000 }); }
    catch (e) {
      await page.keyboard.press('Backquote');
      console.log('     still held: ' + await page.$$eval('#ytc-list .c.fxh', cs => cs.map(c => c.dataset.v).join(' ')) + '\n     ' + (await page.locator('#ytc-debug').textContent()).split('\n').find(l => l.includes('websites ·')));
      throw e;
    }
    const h = await page.evaluate(() => { sessionStorage.removeItem('ytc-e2e-hold'); return window.__hold; });
    assert.ok(h.frames > 10, 'frames sampled');
    assert.deepEqual(h.flashed, [], 'no blocked card was ever displayed');
    const left = await cardIds();
    assert.ok(left.every(id => !FX.linked.has(id)), 'blocked cards removed');
    assert.ok(FX.homeMax <= 2, `concurrent checks ${FX.homeMax}`);
    await page.keyboard.press('Backquote');
    const dbg = (await page.locator('#ytc-debug').textContent()).split('\n').find(l => l.includes('websites ·')) || '';
    await page.keyboard.press('Backquote');
    assert.match(dbg, /held back \d+ \(shown \d+, blocked [1-9]\d*, timed out 0, waiting 0\)/);
    console.log(`     first paint ${first} visible cards · ${dbg.trim()}`);
    // Undo the rule; restore.
    await page.goto('https://www.youtube.com/watch?v=blockLINK01', { waitUntil: 'commit' });
    await page.waitForFunction(() => document.querySelector('#ytc-desc .fx-blk')?.textContent === 'Blocked · Undo');
    await page.click('#ytc-desc .fx-blk');
    await page.waitForFunction(() => document.querySelector('#ytc-desc .fx-blk')?.textContent.startsWith('Block '));
    FX.checkDelay = 0; FX.linked = new Set(); HOME.rich = false;
  });

  await step('resume: the player reports its position; a card shows the local progress bar; reopening resumes there; stored ≤ 2,000 records', async () => {
    await page.goto('https://www.youtube.com/results?search_query=resume+test', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c a.t[href^="/watch"]');
    const v = await page.$eval('#ytc-list .c[data-v]', c => c.dataset.v);
    await page.click(`#ytc-list .c[data-v="${v}"] a.t`);
    await page.waitForFunction(() => { const x = document.querySelector('#ytc-video'); return x && x.currentTime >= 7; }, null, { timeout: 15000 });
    await page.keyboard.press('k'); // pause → position reported
    await page.waitForFunction(() => document.querySelector('#ytc-video').paused);
    const t = await page.$eval('#ytc-video', x => x.currentTime);
    await page.goBack({ waitUntil: 'commit' });
    await page.waitForSelector(`#ytc-list .c[data-v="${v}"] .th i.pg.own`);
    const w = await page.$eval(`#ytc-list .c[data-v="${v}"] .th i.pg.own`, i => parseFloat(i.style.width));
    assert.ok(Math.abs(w - t / SECONDS * 100) < 6, `bar ${w}% for ${t}s of ${SECONDS}s`);
    await page.click(`#ytc-list .c[data-v="${v}"] a.t`);
    await page.waitForFunction(() => +document.documentElement.dataset.ytcFirstFrame > 0 && document.querySelector('#ytc-video')?.readyState >= 2, null, { timeout: 15000 });
    const start = await page.$eval('#ytc-video', x => x.currentTime);
    assert.ok(start >= t - 1, `resumed at ${start}, saved ${t}`);
    await page.keyboard.press('k');
    await page.waitForTimeout(100);
    assert.ok(leveldb().includes('yourTubeProgressV1'));
  });

  await step('watch playlist (list=): panel from next, current row, near-end prefetch, end → next item at once, click jumps; Autoplay countdown, Cancel/Play now, Off remembered', async () => {
    const PL = 'PL_pGtHsOvhtxX3WHo-Hi268L78XBziem_';
    const items = JSON.parse(fx('next_list.json')).contents.twoColumnWatchNextResults.playlist.playlist.contents.map(c => c.playlistPanelVideoRenderer.videoId);
    const nexts = () => log.api.filter(a => a.ep === 'next' && !a.body.continuation && !a.url.includes('fields='));
    await page.goto(`https://www.youtube.com/watch?v=${items[0]}&list=${PL}`, { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-pl .pli.cur');
    assert.ok(nexts().some(a => a.body.videoId === items[0] && a.body.playlistId === PL), 'next asks with the playlist');
    assert.match(await page.locator('#ytc-pl .plm').textContent(), /1 \/ 6$/);
    assert.equal(await page.locator('#ytc-pl .pli').count(), 6);
    assert.equal(await page.locator('#ytc-up-auto').getAttribute('aria-checked'), 'true', 'Autoplay on by default');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    // Near the end (a position report with ≤ 45 s left): the next item's next + VISIONOS, once.
    const n0 = nexts().length;
    await nearEnd(); await keys('k');
    for (let i = 0; i < 40 && !nexts().slice(n0).some(a => a.body.videoId === items[1]); i++) await page.waitForTimeout(50);
    const pre = nexts().slice(n0).filter(a => a.body.videoId === items[1]);
    assert.equal(pre.length, 1, 'next of the next item prefetched once');
    assert.equal(pre[0].body.playlistId, PL);
    assert.ok(log.api.some(a => a.ep === 'player' && a.body.videoId === items[1] && a.body.context.client.clientName === 'VISIONOS'), 'VISIONOS prefetched');
    // End → the next playlist item at once (no countdown), from the prefetch.
    await keys('k');
    await page.waitForFunction(v => location.search.startsWith(`?v=${v}&list=`), items[1], { timeout: 10000 });
    await page.waitForFunction(() => /2 \/ 6$/.test(document.querySelector('#ytc-pl .plm')?.textContent || ''));
    assert.equal(nexts().slice(n0).filter(a => a.body.videoId === items[1]).length, 1, 'the prefetched next was used');
    assert.equal(await page.locator('#ytc-pl .pli.cur .pln').textContent(), '▶');
    if (process.env.YTC_SHOTS) await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'watch-playlist.png') });
    // A row click jumps there.
    const plMark = page.evaluate(() => new Promise(res => {
      const t0 = performance.now();
      const f = () => {
        if (performance.now() - t0 > 8000) return res(null);
        if (!/4 \/ 6$/.test(document.querySelector('#ytc-pl .plm')?.textContent || '')) return requestAnimationFrame(f);
        requestAnimationFrame(() => requestAnimationFrame(() => { const m = document.querySelector('#ytc-pll > i.plk'); res({ ok: !!m, run: m ? m.getAnimations().map(a => a.transitionProperty) : [] }); }));
      };
      requestAnimationFrame(f);
    }));
    await page.click(`#ytc-pl .pli[href*="v=${items[3]}"]`);
    await page.waitForFunction(() => /4 \/ 6$/.test(document.querySelector('#ytc-pl .plm')?.textContent || ''));
    const plm = await plMark;
    assert.ok(plm && plm.ok && plm.run.includes('clip-path'), 'the current-row mark springs from the previous row ' + JSON.stringify(plm));
    await page.waitForTimeout(600);
    const plOn = await page.evaluate(() => { const s = document.getElementById('ytc-pll'), m = s.querySelector(':scope > i.plk'), b = s.querySelector('.pli.cur'), a = s.getBoundingClientRect(), r = b.getBoundingClientRect(), v = (c => { const v = /inset\(([^)]*?)(?: round [^)]*)?\)/.exec(c)[1].trim().split(/\s+/).map(parseFloat); return [v[0], v[1] ?? v[0], v[2] ?? v[0], v[3] ?? v[1] ?? v[0]]; })(getComputedStyle(m).clipPath); return { dt: v[0] - (r.top - a.top + s.scrollTop), dh: (parseFloat(m.style.height) - v[0] - v[2]) - r.height, bg: getComputedStyle(b).backgroundColor }; });
    assert.ok(Math.abs(plOn.dt) < 0.6 && Math.abs(plOn.dh) < 0.6 && plOn.bg === 'rgba(0, 0, 0, 0)', 'the mark lands on the new current row ' + JSON.stringify(plOn));
    assert.ok(page.url().includes(`v=${items[3]}&list=${PL}`));
    // Without a playlist: Autoplay counts down (YouTube's countDownSecs), Cancel stays.
    await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-up-auto');
    assert.equal(await page.locator('#ytc-pl .pl').count(), 0, 'no panel without list=');
    // The Autoplay track ends on the same line as the related cards' bookmark circles (design check, 2 Nov 2026).
    await page.waitForSelector('#ytc-list .c', { state: 'attached' });
    // (the bookmark circle sits flush with its card's right edge, signed in; here the card edge itself)
    const edge = await page.evaluate(() => { const t = document.querySelector('#ytc-up-auto i').getBoundingClientRect(), c = document.querySelector('#ytc-list .c'), w = (c.querySelector('.wl') || c).getBoundingClientRect(); return +(t.right - w.right).toFixed(1); });
    assert.ok(Math.abs(edge) <= 0.5, 'Autoplay track and bookmark share the right edge: ' + edge);
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await nearEnd();
    await page.waitForSelector('#ytc-up .upc', { timeout: 10000 });
    assert.match(await page.locator('#ytc-up .upc').textContent(), /in 5 s/);
    if (process.env.YTC_SHOTS) await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'watch-autoplay.png') });
    if (process.env.YTC_SHOTS) { await page.waitForTimeout(2000); const b = await page.locator('#ytc-up .upc').boundingBox(); await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'upnext-playnow-filling.png'), clip: { x: b.x - 8, y: b.y - 8, width: b.width + 16, height: b.height + 16 } }); }
    await page.click('#ytc-up [data-act=up-cancel]');
    assert.equal(await page.locator('#ytc-up .upc').count(), 0);
    // Play now.
    await nearEnd(); await keys('k');
    await page.waitForSelector('#ytc-up .upc', { timeout: 10000 });
    await page.click('#ytc-up-now');
    await page.waitForFunction(() => location.search === '?v=UURYhAVph5g');
    // Autoplay Off: remembered in ytcConfigV1 (ap:false); the end then stays.
    await page.click('#ytc-up-auto');
    assert.equal(await page.locator('#ytc-up-auto').getAttribute('aria-checked'), 'false');
    await page.waitForTimeout(100);
    assert.equal((await sw.evaluate(() => chrome.storage.local.get('ytcConfigV1'))).ytcConfigV1.ap, false);
    await page.reload({ waitUntil: 'commit' });
    await page.waitForFunction(() => document.getElementById('ytc-up-auto')?.getAttribute('aria-checked') === 'false');
    await waitT(() => document.documentElement.dataset.ytcFirstFrame, null, 8000);
    await nearEnd();
    await waitT(() => document.getElementById('ytc-video').ended, null, 10000);
    await page.waitForTimeout(300);
    assert.equal(await page.locator('#ytc-up .upc').count(), 0);
    assert.equal(new URL(page.url()).search, '?v=UURYhAVph5g');
    await page.click('#ytc-up-auto');
    await page.waitForTimeout(100);
  });

  // YTC_AUDIT=dir: layout audit screenshots (Home with chips signed in, search + Filters, watch page
  // with worst-case data, popovers, player menus, playlist) at 1400×900, 2560×1440 and 900×900.
  // Not a check: skipped unless asked, so the suite's count does not change.
  if (process.env.YTC_AUDIT && !TIMING_ONLY) {
    const dir = process.env.YTC_AUDIT; fs.mkdirSync(dir, { recursive: true });
    const ap = await ctx.newPage();
    const snap = (w, n) => ap.screenshot({ path: path.join(dir, `${w}-${n}.png`), animations: 'disabled' });
    const settle = () => ap.waitForTimeout(350);
    AUDIT.worst = true; ACTS.signedIn = true;
    try {
      for (const [w, h] of [[1400, 900], [2560, 1440], [900, 900]]) {
        await ap.setViewportSize({ width: w, height: h });
        HOME.rich = true; FX.homeChips = true;
        await ap.evaluate(() => sessionStorage.clear()).catch(() => {});
        await ap.goto('https://www.youtube.com/', { waitUntil: 'commit' });
        await ap.waitForSelector('#ytc-list .c[data-v]'); await settle(); await snap(w, 'home');
        HOME.rich = false; FX.homeChips = false;
        await ap.goto('https://www.youtube.com/results?search_query=worst', { waitUntil: 'commit' });
        await ap.waitForSelector('#ytc-list .c[data-v]'); await settle(); await snap(w, 'search');
        await ap.hover('#ytc-hfx .fx-fb'); await settle(); await snap(w, 'search-filters'); await ap.mouse.move(5, h - 5);
        await ap.goto(`https://www.youtube.com/watch?v=${WORST_ID}`, { waitUntil: 'commit' });
        await ap.waitForSelector('#ytc-list .c[data-v]'); await ap.waitForSelector('#ytc-acts .ya-b'); await settle();
        await ap.evaluate(() => { const v = document.getElementById('ytc-video'); if (v) v.pause(); });
        await snap(w, 'watch');
        for (const [b, n] of [['#ytc-save-b', 'save'], ['#ytc-share-b', 'share'], ['#ytc-nt-b', 'bell'], ['#ytc-ac-b', 'account']]) {
          if (!(await ap.locator(b).count())) continue;
          await ap.click(b); await ap.waitForTimeout(700); await snap(w, n); await ap.keyboard.press('Escape'); await ap.mouse.click(5, h - 5); await ap.waitForTimeout(200);
        }
        for (const [b, n] of [['#ytc-speed', 'menu-speed'], ['#ytc-qual', 'menu-quality'], ['#ytc-ccb', 'menu-cc']]) {
          await ap.click(b); await ap.waitForTimeout(300); await snap(w, n); await ap.click(b); await ap.waitForTimeout(200);
        }
        await ap.evaluate(() => scrollTo(0, document.getElementById('ytc-comments').getBoundingClientRect().top + scrollY - 200));
        await ap.waitForSelector('#ytc-comments .ycm', { timeout: 5000 }).catch(() => {}); await settle(); await snap(w, 'comments');
        const PL = 'PL_pGtHsOvhtxX3WHo-Hi268L78XBziem_';
        const items = JSON.parse(fx('next_list.json')).contents.twoColumnWatchNextResults.playlist.playlist.contents.map(c => c.playlistPanelVideoRenderer.videoId);
        await ap.goto(`https://www.youtube.com/watch?v=${items[0]}&list=${PL}`, { waitUntil: 'commit' });
        await ap.waitForSelector('#ytc-pl .pli.cur'); await settle();
        await ap.evaluate(() => { const v = document.getElementById('ytc-video'); if (v) v.pause(); });
        await snap(w, 'watch-playlist');
        await ap.goto(`https://www.youtube.com/playlist?list=${PL}`, { waitUntil: 'commit' });
        await ap.waitForSelector('#ytc-list .c'); await settle(); await snap(w, 'playlist');
      }
    } finally { AUDIT.worst = false; ACTS.signedIn = false; HOME.rich = false; FX.homeChips = false; await ap.close(); }
    console.log(`     audit screenshots in ${dir}`);
  }


  // ---- Voice search (2 Nov 2026): hover the mic or hold Option to dictate with Deepgram (mocked here).
  const micGeo = () => page.evaluate(() => {
    const m = document.getElementById('ytc-mic'), b = m.getBoundingClientRect(), i = m.querySelector('svg').getBoundingClientRect();
    return { x: b.left, y: b.top, w: b.width, h: b.height, cx: b.right - 18, cy: b.top + 18, ix: i.left + i.width / 2, iy: i.top + i.height / 2, v: document.getElementById('ytc-search').dataset.v || '', clip: getComputedStyle(m).clipPath };
  });
  const hitMic = (x, y) => page.evaluate(([x, y]) => !!document.elementFromPoint(x, y)?.closest('#ytc-mic'), [x, y]);
  const voicePage = async q => {
    await page.goto('https://www.youtube.com/results?search_query=' + q, { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.mouse.move(40, 860);
    await page.waitForTimeout(150);
  };
  const lastConn = () => dg.conns[dg.conns.length - 1];
  await step('voice: no key → resting on the mic explains in one line (nothing opened); the key is saved from the panel without entering the page', async () => {
    await voicePage('voice');
    const g = await micGeo();
    assert.ok(Math.abs(g.ix - g.cx) <= 1 && Math.abs(g.iy - g.cy) <= 1, 'mic glyph centred in its circle ' + JSON.stringify(g));
    assert.ok(!(await hitMic(g.x + 10, g.y + 18)), 'at rest the hover target is only the circle');
    await page.mouse.move(g.cx, g.cy);
    await page.waitForSelector('#ytc-vk.on', { timeout: 2000 });
    assert.equal(await page.locator('#ytc-vk-m').textContent(), 'Paste your Deepgram API key to search by voice.');
    assert.equal(dg.conns.length, 0, 'no socket without a key');
    assert.equal((await micGeo()).v, '', 'not listening');
    await page.click('#ytc-vk-in');
    await page.keyboard.type(VKEY);
    await page.keyboard.press('Enter');
    await waitT(() => /Key saved/.test(document.getElementById('ytc-vk-m')?.textContent || ''), null, 2000);
    assert.equal(await page.inputValue('#ytc-vk-in'), '', 'the field is emptied');
    assert.equal((await sw.evaluate(() => chrome.storage.local.get('ytcVoiceKeyV1'))).ytcVoiceKeyV1, VKEY);
    assert.ok(!(await page.content()).includes(VKEY), 'the key is not in the page');
    await page.mouse.click(40, 860);
    await page.waitForSelector('#ytc-vk:not(.on)', { state: 'attached' });
  });
  await step('voice: a short hover does nothing; resting 200 ms morphs the mic into a bigger listening pill (live level); interim words muted, finals normal; hover-out searches', async () => {
    await voicePage('voice');
    let g = await micGeo();
    const n0 = dg.conns.length;
    await page.mouse.move(g.cx, g.cy);
    await page.waitForTimeout(120);
    await page.mouse.move(40, 860);
    await page.waitForTimeout(300);
    assert.equal(dg.conns.length, n0, 'leaving within 200 ms opens nothing');
    assert.equal((await micGeo()).v, '');
    // Rest on it.
    const t0 = Date.now();
    await page.mouse.move(g.cx, g.cy);
    await page.waitForFunction(() => document.getElementById('ytc-search').dataset.v === 'on', null, { timeout: 2000 });
    assert.ok(Date.now() - t0 >= 180, 'starts after the dwell');
    // Mid-morph: the circle is stretching (the spring), the pointer still on it.
    const mid = await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(getComputedStyle(document.getElementById('ytc-mic')).clipPath)))));
    assert.notEqual(mid, 'inset(0px round 18px)', 'the pill grows on the spring, not at once: ' + mid);
    if (process.env.YTC_SHOTS) await page.screenshot({ path: path.join(process.env.YTC_SHOTS, 'voice-mid-morph.png'), clip: { x: g.x - 600, y: 0, width: 760, height: 64 } });
    await page.waitForTimeout(450);
    g = await micGeo();
    assert.equal(g.clip, 'inset(0px round 18px)', 'the listening pill');
    assert.ok(await hitMic(g.x + 10, g.y + 18), 'the pill is a bigger hover target');
    await waitT(() => !!document.getElementById('ytc-qv')?.textContent.includes('Listening'), null, 2000);
    const c = lastConn();
    assert.ok(c && c.protos[0] === 'token' && c.protos[1] === VKEY, 'Sec-WebSocket-Protocol: token, KEY');
    const u = new URL(c.url, 'http://x');
    for (const [k, v] of [['model', 'nova-3'], ['language', 'multi'], ['smart_format', 'true'], ['interim_results', 'true'], ['endpointing', '300'], ['encoding', 'linear16'], ['sample_rate', '16000'], ['channels', '1']]) assert.equal(u.searchParams.get(k), v, k);
    await waitT(() => true, null, 100);
    for (let i = 0; i < 30 && c.bins < 5; i++) await page.waitForTimeout(50);
    assert.ok(c.bins >= 5 && c.bytes % 2 === 0, `16-bit PCM chunks stream (${c.bins} chunks, ${c.bytes} bytes)`);
    // The fake microphone beeps: the level property moves.
    const b0 = c.bins, w0 = Date.now();
    const lv = await page.evaluate(() => new Promise(r => { const s = new Set(), t = setInterval(() => s.add(document.getElementById('ytc-mic').style.getPropertyValue('--lv')), 20); setTimeout(() => { clearInterval(t); r([...s]); }, 1300); }));
    assert.ok(lv.some(v => +v > 0.1), 'the waveform follows the level: ' + lv.join(' '));
    // 512 samples per chunk at 16 kHz (the rate the URL declares): ~31 chunks a second, 1024 bytes each.
    const rate = (c.bins - b0) / ((Date.now() - w0) / 1000);
    assert.ok(rate > 24 && rate < 40 && c.bytes / c.bins === 1024, `16 kHz chunks: ${rate.toFixed(1)}/s, ${c.bytes / c.bins} bytes`);
    c.send(wsMock.result('Roblox scrip'));
    await waitT(() => document.querySelector('#ytc-qv i')?.textContent === 'Roblox scrip', null, 2000);
    c.send(wsMock.result('Roblox scripting', { final: true }));
    c.send(wsMock.result('tutor'));
    await waitT(() => document.querySelector('#ytc-qv b')?.textContent === 'Roblox scripting' && document.querySelector('#ytc-qv i')?.textContent === 'tutor', null, 2000);
    const col = await page.evaluate(() => ({ b: getComputedStyle(document.querySelector('#ytc-qv b')).color, i: getComputedStyle(document.querySelector('#ytc-qv i')).color, q: document.getElementById('ytc-q').value }));
    assert.deepEqual(col, { b: 'rgb(237, 243, 243)', i: 'rgb(160, 175, 177)', q: 'Roblox scripting tutor' }, 'finals normal, interim muted');
    c.send(wsMock.result('tutorial.', { final: true, speech: true }));
    await waitT(() => document.querySelector('#ytc-qv b')?.textContent === 'Roblox scripting tutorial.', null, 2000);
    // Settled: hover-out searches at once (no flush needed); the results were warmed meanwhile.
    const before = log.api.length;
    await page.mouse.move(g.x + 10, g.y + 70);
    await page.waitForFunction(() => new URLSearchParams(location.search).get('search_query') === 'Roblox scripting tutorial', null, { timeout: 3000 });
    const d = await ds();
    console.log(`     voice hover-out → search ${(+d.ytcVoiceSearch - +d.ytcVoiceEnd).toFixed(1)} ms (settled transcript, mocked Deepgram)`);
    assert.ok(log.api.slice(before - 3).some(a => a.ep === 'search' && a.body.query === 'Roblox scripting tutorial') || log.api.some(a => a.ep === 'search' && a.body.query === 'Roblox scripting tutorial'), 'searched');
    await waitT(() => !document.getElementById('ytc-search').dataset.v, null, 1000);
    for (let i = 0; i < 20 && !c.gone; i++) await page.waitForTimeout(50);
    assert.ok(c.gone, 'the socket closes with the dictation');
    const bins = c.bins; await page.waitForTimeout(300);
    assert.equal(c.bins, bins, 'no audio after the end');
  });
  await step('voice: push-to-talk (hold Option): a tap or a chord opens nothing; held, the pill shows at once; release flushes and searches; Escape cancels', async () => {
    await voicePage('voice');
    const n0 = dg.conns.length;
    // A tap, and Option+key chords (typing, Alt+←), never open the microphone.
    await page.keyboard.down('Alt'); await page.waitForTimeout(80); await page.keyboard.up('Alt');
    await page.keyboard.down('Alt'); await page.waitForTimeout(100); await page.keyboard.press('KeyX'); await page.waitForTimeout(300); await page.keyboard.up('Alt');
    await page.waitForTimeout(200);
    assert.equal(dg.conns.length, n0, 'no socket for a tap or a chord');
    dg.onconn = c => { c.ontext = t => { if (t.includes('Finalize')) c.send(wsMock.result('Лучшие игры Roblox.', { final: true, flush: true })); }; };
    await page.keyboard.down('Alt');
    await page.waitForFunction(() => document.getElementById('ytc-search').dataset.v === 'key', null, { timeout: 2000 });
    const inst = await page.evaluate(() => new Promise(r => requestAnimationFrame(() => r(getComputedStyle(document.getElementById('ytc-mic')).clipPath))));
    assert.equal(inst, 'inset(0px round 18px)', 'keyboard: the pill is there at once');
    for (let i = 0; i < 40 && dg.conns.length === n0; i++) await page.waitForTimeout(25);
    const c = lastConn();
    c.send(wsMock.result('лучшие игры'));
    await waitT(() => document.querySelector('#ytc-qv i')?.textContent === 'лучшие игры', null, 2000);
    await page.keyboard.up('Alt');
    await page.waitForFunction(() => new URLSearchParams(location.search).get('search_query') === 'Лучшие игры Roblox', null, { timeout: 3000 });
    assert.ok(c.texts.includes('{"type":"Finalize"}') && c.texts.includes('{"type":"CloseStream"}'), 'flushed: ' + c.texts.join());
    const d = await ds();
    console.log(`     voice key release → search ${(+d.ytcVoiceSearch - +d.ytcVoiceEnd).toFixed(1)} ms (incl. the 60 ms microphone tail and the Finalize round trip, mocked)`);
    dg.onconn = null;
    // Escape (hover dictation) cancels: nothing searched, the field restored, the socket closed.
    await voicePage('keepme');
    const g = await micGeo();
    await page.mouse.move(g.cx, g.cy);
    await page.waitForFunction(() => document.getElementById('ytc-search').dataset.v === 'on', null, { timeout: 2000 });
    for (let i = 0; i < 40 && !(lastConn() && !lastConn().gone && lastConn().bins); i++) await page.waitForTimeout(25);
    const e = lastConn();
    e.send(wsMock.result('cancel me'));
    await waitT(() => document.querySelector('#ytc-qv i')?.textContent === 'cancel me', null, 2000);
    await page.keyboard.press('Escape');
    await waitT(() => !document.getElementById('ytc-search').dataset.v, null, 1000);
    assert.equal(await page.inputValue('#ytc-q'), 'keepme', 'field restored');
    for (let i = 0; i < 20 && !e.gone; i++) await page.waitForTimeout(50);
    assert.ok(e.gone, 'socket closed');
    await page.waitForTimeout(400);
    assert.equal(new URLSearchParams(new URL(page.url()).search).get('search_query'), 'keepme', 'nothing searched');
    assert.equal((await micGeo()).v, '', 'no new dwell while the pointer stays');
    await page.mouse.move(40, 860);
    // Navigation while listening stops it too.
    await page.keyboard.down('Alt');
    await page.waitForFunction(() => document.getElementById('ytc-search').dataset.v === 'key', null, { timeout: 2000 });
    const f = lastConn();
    await page.keyboard.up('Alt'); // nothing heard: the release ends quietly after the flush
    await waitT(() => !document.getElementById('ytc-search').dataset.v, null, 1000);
    for (let i = 0; i < 40 && !f.gone; i++) await page.waitForTimeout(50);
    assert.ok(f.gone, 'socket closed after an empty dictation');
    assert.equal(new URLSearchParams(new URL(page.url()).search).get('search_query'), 'keepme', 'an empty dictation searches nothing');
  });
  await step('voice: a rejected key explains itself; the key never appears in other storage, incidents, the page or the debug overlay; ? lists the key', async () => {
    await voicePage('voice');
    await sw.evaluate(() => chrome.storage.local.set({ ytcVoiceKeyV1: 'wrongKey123' }));
    const g = await micGeo();
    await page.mouse.move(g.cx, g.cy);
    await waitT(() => /rejected this key/.test(document.getElementById('ytc-vk-m')?.textContent || ''), null, 4000);
    assert.equal((await micGeo()).v, '', 'back at rest');
    assert.ok(dg.probes.some(p => p.path === '/v1/projects' && !p.ok), 'the probe told a bad key from a network error');
    await sw.evaluate(k => chrome.storage.local.set({ ytcVoiceKeyV1: k }), VKEY);
    await page.mouse.move(40, 860);
    await page.mouse.click(40, 860);
    const all = await sw.evaluate(() => chrome.storage.local.get(null));
    const leaks = Object.entries(all).filter(([k, v]) => k !== 'ytcVoiceKeyV1' && JSON.stringify(v).includes(VKEY)).map(([k]) => k);
    assert.deepEqual(leaks, [], 'the key only under its own storage key');
    assert.ok(!JSON.stringify(incidents).includes(VKEY), 'not in incidents');
    const pg = await page.evaluate(k => [document.documentElement.outerHTML, JSON.stringify(localStorage), JSON.stringify(sessionStorage)].some(x => x.includes(k)), VKEY);
    assert.ok(!pg, 'not in the page, localStorage or sessionStorage');
    await page.keyboard.press('Backquote');
    const dbg = await page.locator('#ytc-debug').textContent();
    await page.keyboard.press('Backquote');
    assert.ok(/voice · key set/.test(dbg) && !dbg.includes(VKEY), 'debug overlay: state only');
    await page.keyboard.press('?');
    assert.ok((await page.locator('#ytc-keys').textContent()).includes('Voice search'), '? sheet lists the voice key');
    await page.keyboard.press('Escape');
    // No sound: nothing is played (no media element besides the player's, which this page has none of).
    assert.equal(await page.locator('audio').count(), 0);
  });
  await step('import from the old extension: console line + Copy, strict validation (a bad paste changes nothing), merge, applied live (chips, topics, filters, history, resume bar, captions), stored; Escape closes', async () => {
    const OLD = fixture('old_extension_storage.json').toString('utf8');
    const keys = ['yourTubeHistoryV1', 'yourTubeProgressV1', 'yourTubeBlockedWebsitesV1', 'yourTubeRecommendationTopicsV1', 'yourTubeNativeTopicsV1', 'yourTubeMinimumHomeDurationV1', 'yourTubeHomeOrderV1', 'yourTubeHomePlaylistsOnlyV1', 'yourTubeShowMusicV1', 'yourTubeShowLiveV1', 'shortsInFeeds', 'ytcConfigV1'];
    const before = await sw.evaluate(k => chrome.storage.local.get(k), keys);
    HOME.rich = true; FX.homeChips = true;
    try {
      await page.evaluate(() => sessionStorage.clear());
      await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-filters [data-act=fx-nchip]');
      await page.hover('#ytc-filters .fx-plus');
      await page.waitForTimeout(300);
      await page.click('#ytc-fx-imp');
      await page.waitForSelector('#ytc-imp.on .imp-p');
      assert.equal(await page.evaluate(() => document.activeElement?.id), 'ytc-imp-in', 'focus in the paste box');
      assert.equal(await page.inputValue('#ytc-imp-cmd'), 'chrome.storage.local.get(null, d => copy(JSON.stringify(d)))');
      await page.click('#ytc-imp [data-act=fx-imp-copy]');
      assert.match(await page.locator('#ytc-imp-m').textContent(), /Copied|copy it/);
      await page.mouse.move(5, 5);
      await shot('import-dialog');
      // A broken or foreign paste is refused whole; nothing is written.
      const n0 = await sw.evaluate(() => chrome.storage.local.get(null).then(o => JSON.stringify(o).length));
      for (const bad of ['{"yourTubeHistoryV1": {"version": 1, "entries": []', '{"yourTubeHistoryV1": {"version": 1, "entries": []}, "yourTubeHomeOrderV1": "newest"}', '{"other": 1}']) {
        await page.fill('#ytc-imp-in', bad);
        await page.click('#ytc-imp [data-act=fx-imp-go]');
        await page.waitForSelector('#ytc-imp-m.er');
        await page.evaluate(() => { document.getElementById('ytc-imp-m').className = 'imp-m'; });
      }
      assert.equal(await sw.evaluate(() => chrome.storage.local.get(null).then(o => JSON.stringify(o).length)), n0, 'nothing written by a rejected paste');
      await page.fill('#ytc-imp-in', OLD);
      await page.click('#ytc-imp [data-act=fx-imp-go]');
      await page.waitForSelector('#ytc-imp-m.ok');
      const summary = await page.locator('#ytc-imp-m').textContent();
      assert.match(summary, /^Imported 87 searches, 1,204 resume positions, 2 blocked websites, 2 topics, 2 YouTube topic settings, Home filter settings and caption size, position and on\/off\. 1 record skipped \(invalid or over the limits\)\. Viewing statistics and the old topic lists are not carried over\.$/, summary);
      assert.equal(await page.inputValue('#ytc-imp-in'), '', 'the paste box is emptied');
      await shot('import-done');
      // Applied live, no reload: YouTube's chip renamed, the topics and the length filter are there.
      assert.equal(await page.locator('#ytc-fx-nchip-gaming').textContent(), 'Games');
      const chips = await page.$$eval('#ytc-filters .fx-tc .fx-ch', bs => bs.map(b => b.textContent));
      assert.ok(chips.includes('Roblox scripting') && chips.includes('Woodworking joints'), chips.join('|'));
      assert.match(await page.locator('#ytc-hfx').textContent(), /20\+ min/);
      // Escape closes the dialog at once.
      await page.focus('#ytc-imp-in');
      await page.keyboard.press('Escape');
      await page.waitForSelector('#ytc-imp:not(.on)', { state: 'attached' });
      assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-imp')).visibility), 'hidden');
      // Stored in the client's own keys; statistics and the legacy topic stores are left out.
      const st = await sw.evaluate(() => chrome.storage.local.get(null));
      assert.ok(st.yourTubeHistoryV1.entries.some(e => e.query === 'old extension search'));
      assert.ok(Object.keys(st.yourTubeProgressV1.entries).length >= 1204);
      assert.deepEqual(st.yourTubeBlockedWebsitesV1.domains.slice(-2), ['spam.example.com', 'clickbait.example.org']);
      assert.equal(st.yourTubeMinimumHomeDurationV1, 20);
      assert.equal(st.yourTubeShowMusicV1, true);
      assert.deepEqual(st.ytcConfigV1.p.cs, { v: 2, x: 0.1, y: 0.7, s: 1.25 });
      assert.equal(st.ytcConfigV1.p.cc, false);
      for (const k of ['yourTubeStatsV1', 'yourTubeDiscoveryTopicsV1', 'yourTubeHomeTopicsV1']) assert.ok(!(k in st), k + ' not stored');
      // The search box offers the imported searches; a card shows the imported resume position.
      await page.click('#ytc-q');
      await page.keyboard.type('old exten');
      await page.waitForFunction(() => document.getElementById('ytc-sug')?.textContent.includes('old extension search'));
      await page.keyboard.press('Escape');
      await page.evaluate(() => document.activeElement?.blur());
      await page.goto('https://www.youtube.com/results?search_query=imported', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v="zSK7bOoxuig"] .pg.own');
      assert.equal(await page.getAttribute('#ytc-list .c[data-v="zSK7bOoxuig"] .pg.own', 'style'), 'width:50%');
      // The player takes the caption settings: size, place, and off.
      await page.goto('https://www.youtube.com/watch?v=-Bxc4gJluC0', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-video');
      await page.waitForFunction(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cs') === '1.250');
      assert.equal(await page.evaluate(() => getComputedStyle(document.getElementById('ytc-player')).getPropertyValue('--cx')), '0.1000');
      assert.ok(!(await page.getAttribute('#ytc-player', 'class')).split(' ').includes('cc'), 'captions off');
    } finally {
      HOME.rich = false; FX.homeChips = false;
      await sw.evaluate(async ([keys, before]) => { await chrome.storage.local.remove(keys); await chrome.storage.local.set(before); }, [keys, before]);
    }
  });

  await step('perf: timings reach the debug center as kind "perf" (numbers only, no tokens/cookies/ids)', async () => {
    await page.goto('https://www.youtube.com/results?search_query=perf+check', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    const vid = new URL(await page.locator('#ytc-list a.t[href^="/watch"]').first().getAttribute('href'), 'https://www.youtube.com').searchParams.get('v');
    await page.click('#ytc-list a.t[href^="/watch"]');
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.evaluate(() => sessionStorage.removeItem('ytc:perfat'));
    await page.goto('https://www.youtube.com/results?search_query=after', { waitUntil: 'commit' }); // leaving the document reports (≥ 1 min apart)
    for (let i = 0; i < 50 && !incidents.some(x => x.kind === 'perf' && x.perf?.ms?.['nav:watch-related']); i++) await page.waitForTimeout(100);
    const r = incidents.filter(x => x.kind === 'perf' && x.perf?.ms?.['nav:watch-related']).at(-1);
    assert.ok(r, 'perf report delivered');
    assert.ok(r.perf.ms.next?.p50 >= 0 && r.perf.ms['nav:watch-related']?.n >= 1, JSON.stringify(r.perf.ms));
    assert.ok(r.perf.cache['watch-next'], 'cache hit rates');
    const str = JSON.stringify(r);
    assert.ok(!/SAPISID|continuation|token|search_query|perf\+check/.test(str) && !str.includes(vid), str.slice(0, 300));
  });

  await step('public build: no speed telemetry, nothing kept for a collector, no development permission or auto-reload', async () => {
    if (DEV_BUILD) return;
    assert.equal(telBatches(0).length, 0, 'no telemetry batch in the whole run');
    const st = await sw.evaluate(() => chrome.storage.local.get(null));
    assert.ok(!('ytcIncidentsV1' in st) && !('ytcPerfQueueV1' in st), 'nothing stored for a collector: ' + Object.keys(st).join(','));
    const m = await sw.evaluate(() => chrome.runtime.getManifest());
    assert.ok(!m.host_permissions.some(p => p.includes('127.0.0.1')) && !m.version_name, 'public manifest');
    const files = ['content.js', 'background.js', 'manifest.json'].map(f => fs.readFileSync(path.join(EXT, f), 'utf8')).join('\n');
    for (const s of ['127.0.0.1:47321', 'ytc-dev-check', 'build-id.txt', 'ytcIncidentsV1', 'ytcPerfQueueV1']) assert.ok(!files.includes(s), 'no ' + s + ' in the public build');
  });
  await step('dev build: perf telemetry: a navigation and a video start arrive as schema-2 events with plausible numbers and no secrets', async () => {
    const from = incidents.length;
    await ctx.addCookies([{ name: 'SAPISID', value: 'telSECRETsapisid0', domain: '.youtube.com', path: '/' }]);
    try {
      await page.goto('https://www.youtube.com/results?search_query=tel+check+secret', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      const vd = await page.evaluate(() => { try { return JSON.parse(sessionStorage.getItem('ytc:cfg')).vd || ''; } catch { return ''; } });
      const a = page.locator('#ytc-list a.t[href^="/watch"]').nth(2);
      const vid = new URL(await a.getAttribute('href'), 'https://www.youtube.com').searchParams.get('v');
      const t0 = await page.evaluate(() => performance.now());
      await a.click();
      await page.waitForFunction(t => +document.documentElement.dataset.ytcFirstFrame > t, t0, { timeout: 8000 });
      await page.waitForTimeout(1200);
      await page.keyboard.press('j'); // a seek (and its toast)
      await page.waitForTimeout(800);
      await page.goto('https://www.youtube.com/results?search_query=after+tel', { waitUntil: 'commit' }); // leaving sends the batch
      for (let i = 0; i < 50 && !telBatches(from).some(b => b.events.some(e => e.m === 'video.session' && e.vid === vid)); i++) await page.waitForTimeout(100);
      const batches = telBatches(from), ev = batches.flatMap(b => b.events);
      const by = m => ev.filter(e => e.m === m);
      const counts = {}; for (const e of ev) counts[e.m] = (counts[e.m] || 0) + 1;
      console.log('     telemetry events: ' + JSON.stringify(counts) + ` · overhead ${batches.reduce((s, b) => s + b.overhead_ms, 0).toFixed(1)} ms Rust + ${batches.reduce((s, b) => s + b.js_overhead_ms, 0).toFixed(1)} ms observers`);
      assert.ok(batches.length, 'batch delivered');
      for (const b of batches) {
        assert.ok(b.build && /^\d+\.\d+\.\d+ \([0-9a-f]{16}\)$/.test(b.build), 'build stamped: ' + b.build);
        assert.ok(b.events.length <= 200 && b.page && b.seq >= 1 && b.overhead_ms >= 0, JSON.stringify({ ...b, events: b.events.length }));
        for (const e of b.events) assert.ok(typeof e.m === 'string' && Number.isFinite(e.v) && e.at > 1.7e12, JSON.stringify(e));
      }
      const boot = by('nav.search').find(e => e.mode === 'boot');
      assert.ok(boot && boot.input === 'boot' && boot.v > 0 && boot.v < 5000 && boot.reqs?.some(r => r.ep === 'search' && r.status === 200 && r.bytes > 1000), 'boot navigation ' + JSON.stringify(boot));
      const nav = by('nav.watch').find(e => e.mode === 'push');
      assert.ok(nav && nav.input === 'pointer' && ['cache', 'inflight', 'network', 'prefetch'].includes(nav.src), 'watch navigation ' + JSON.stringify(nav));
      assert.ok(nav.v > 0 && nav.v < 3000 && nav.insert_ms <= nav.v && nav.handler_ms >= 0, 'watch navigation numbers ' + JSON.stringify(nav));
      const search = by('req.search')[0];
      assert.ok(search && search.status === 200 && search.bytes > 1000 && search.v >= 0 && search.via === 'page', 'req.search ' + JSON.stringify(search));
      const start = by('video.start').find(e => e.vid === vid);
      assert.ok(start && start.v > 0 && start.v < 8000 && start.input === 'navigation', 'video.start ' + JSON.stringify(start));
      assert.ok(start.gv.length > 0 && start.gv.every(r => [200, 206].includes(r.status) && r.itag > 0 && r.bytes > 0), 'googlevideo requests in the start breakdown');
      assert.ok(['cache', 'prefetch', 'fetch'].includes(start.vision.src) && (start.watch_ms == null || start.watch_ms <= start.v) && start.mse_open_ms >= 0, 'start breakdown ' + JSON.stringify(start));
      // Straight to the target rung; the slow-YouTube breakdown is there (nothing fired on local media).
      assert.ok(start.start_p === start.target_p, 'no low start rung ' + JSON.stringify(start));
      assert.deepEqual(start.slow, { vision_hedge: { fired: false, won: false }, hedge_fired: 0, hedge_won: 0, throttle_detected: 0, path: [] }, 'slow breakdown');
      const head = by('gv.head').find(e => e.vid === vid);
      assert.ok(head && /googlevideo\.com$/.test(head.host) && head.itag > 0 && head.bytes > 0 && head.v >= 0, 'gv.head ' + JSON.stringify(head));
      const seek = by('video.seek').find(e => e.vid === vid);
      assert.ok(seek && seek.v >= 0 && seek.v < 5000 && seek.input === 'keyboard', 'video.seek ' + JSON.stringify(seek));
      assert.ok(by('ui.toast').some(e => e.v >= 0 && e.v < 2000), 'toast latency');
      const session = by('video.session').find(e => e.vid === vid);
      assert.ok(session && session.started && session.seeks >= 1 && session.v > 1000, 'video.session ' + JSON.stringify(session));
      assert.ok(by('lag.cls').some(e => e.route === 'search' && e.v >= 0), 'layout shift of the page left');
      const str = JSON.stringify(batches);
      for (const secret of ['telSECRETsapisid0', 'SAPISID', FRESH_VD, vd, 'tel check', 'tel+check', 'search_query', 'videoplayback', 'sig=', 'expire=', 'authorization', 'x-goog'].filter(Boolean))
        assert.ok(!str.includes(secret), 'no ' + secret + ' in telemetry');
    } finally { await ctx.clearCookies(); }
    // Switched off (ytcTelemetryV1: false): nothing is recorded or sent.
    await sw.evaluate(() => chrome.storage.local.set({ ytcTelemetryV1: false }));
    try {
      // (The page open while it was switched off still sends what it recorded when it is left.)
      await page.goto('https://www.youtube.com/results?search_query=tel+off', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.waitForTimeout(1500);
      const off = incidents.length;
      await page.click('#ytc-list a.t[href^="/watch"]');
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.goto('https://www.youtube.com/results?search_query=tel+off+2', { waitUntil: 'commit' });
      await page.waitForSelector('#ytc-list .c[data-v]');
      await page.waitForTimeout(1500);
      assert.equal(telBatches(off).length, 0, 'switched off: no telemetry batch');
    } finally { await sw.evaluate(() => chrome.storage.local.remove('ytcTelemetryV1')); }
  });

  await step('dev build: interaction log: clicks, handled keys, the search box, back and a hover dwell become ui.act events (input → handler → DOM → paint, what they waited on), with no text or search terms', async () => {
    const from = incidents.length;
    await page.goto('https://www.youtube.com/results?search_query=act+log+secret', { waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.waitForTimeout(400);
    const a = page.locator('#ytc-list a.th[href^="/watch"]').nth(1);
    const vid = new URL(await a.getAttribute('href'), 'https://www.youtube.com').searchParams.get('v');
    await a.click();
    await page.waitForSelector('#ytc-player');
    await page.waitForTimeout(600);
    await page.keyboard.press('k'); // pause
    await page.waitForTimeout(200);
    await page.goBack({ waitUntil: 'commit' });
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.waitForTimeout(300);
    await page.click('#ytc-q');
    await page.keyboard.type('typed words', { delay: 10 });
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => location.search.includes('typed'));
    await page.waitForSelector('#ytc-list .c[data-v]');
    await page.waitForTimeout(300);
    const box = await page.locator('#ytc-logo').boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 2 });
    await page.waitForFunction(() => location.pathname === '/', null, { timeout: 3000 });
    await page.waitForTimeout(500);
    await page.mouse.move(700, 600);
    await page.goto('https://www.youtube.com/results?search_query=after+acts', { waitUntil: 'commit' });
    for (let i = 0; i < 50 && !telBatches(from).some(b => b.events.some(e => e.m === 'ui.act' && e.act === 'logo.home')); i++) await page.waitForTimeout(100);
    const ev = telBatches(from).flatMap(b => b.events), acts = ev.filter(e => e.m === 'ui.act');
    console.log('     interactions: ' + acts.map(e => `${e.act} ${e.input} ${e.v}${e.content_ms != null ? '/' + e.content_ms : ''} ms (${e.wait})`).join(' · '));
    for (const e of acts) {
      assert.ok(e.v >= 0 && e.v < 5000 && e.handler_ms >= 0 && e.handler_ms <= e.v + 0.1 && e.t > 0 && e.at > 1.7e12 && e.route != null && typeof e.nav === 'boolean', 'numbers ' + JSON.stringify(e));
      assert.ok(e.dom_ms == null || e.dom_ms >= e.handler_ms - 0.1, 'dom after the handler ' + JSON.stringify(e));
      assert.ok(/^[a-z0-9.\-_\/?]+$/.test(e.act), 'a code name ' + e.act);
    }
    const card = acts.find(e => e.act === 'card.open');
    assert.ok(card && card.input === 'pointer' && card.nav && card.vid === vid && card.to === 'watch' && card.kind === 'a@list' && card.route === 'search', 'card.open ' + JSON.stringify(card));
    assert.ok(['cache', 'prefetch', 'inflight', 'network', 'memory'].includes(card.wait) && card.content_ms >= card.dom_ms - 0.1, 'card.open waited on ' + JSON.stringify(card));
    assert.ok(ev.some(e => e.m === 'nav.watch' && e.act === 'card.open'), 'the navigation names its interaction');
    const k = acts.find(e => e.act === 'key.k');
    assert.ok(k && k.input === 'keyboard' && k.route === 'watch' && !k.nav && k.wait === 'none', 'key.k ' + JSON.stringify(k));
    const back = acts.find(e => e.act === 'history.pop');
    assert.ok(back && back.input === 'history' && back.nav && back.content_ms >= 0, 'history.pop ' + JSON.stringify(back));
    const sub = acts.find(e => e.act === 'search.submit');
    assert.ok(sub && sub.input === 'keyboard' && sub.nav, 'search.submit ' + JSON.stringify(sub));
    const logo = acts.find(e => e.act === 'logo.home');
    assert.ok(logo && logo.input === 'hover' && logo.nav, 'logo dwell ' + JSON.stringify(logo));
    assert.ok(!acts.some(e => /^key\.[a-z]$/.test(e.act) && e.act !== 'key.k'), 'typed letters are not interactions');
    const str = JSON.stringify(acts);
    for (const secret of ['typed', 'act log', 'act+log', 'secret', 'search_query', '/watch?']) assert.ok(!str.includes(secret), 'no ' + secret + ' in the interaction log');
    const boot = ev.find(e => e.m === 'nav.search' && e.mode === 'boot');
    assert.ok(boot && boot.boot && boot.boot.response_start > 0 && boot.boot.script >= boot.boot.response_start && boot.boot.wasm >= boot.boot.script && boot.boot.app >= boot.boot.wasm, 'boot breakdown ' + JSON.stringify(boot && boot.boot));
  });

  await step('TV mode (the TV app\'s platform flag, here without its shell): D-pad walks the grid, OK opens our player, ↓ enters the control bar, no autofocus on Home', async () => {
    // The TV app sets YTC_PLATFORM; localStorage ytc:platform is the development switch for the same mode.
    const tv = await ctx.newPage();
    const tvErrors = [];
    tv.on('pageerror', e => tvErrors.push(String(e)));
    try {
      await tv.addInitScript(() => { try { localStorage.setItem('ytc:platform', 'tv'); } catch {} });
      await tv.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await tv.waitForSelector('.hero');
      assert.equal(await tv.getAttribute('html', 'data-ytc-tv'), '1');
      assert.equal(await tv.locator('.hero input[autofocus]').count(), 0, 'no autofocus: the TV keyboard would take the arrows');
      assert.equal(await tv.locator('.hero [data-act=tv-signin]').count(), 1, 'device sign-in instead of the Google page');
      await tv.goto('https://www.youtube.com/results?search_query=lofi', { waitUntil: 'commit' });
      await tv.waitForSelector('#ytc-list .c a.t');
      const focused = () => tv.evaluate(() => { const a = document.activeElement, r = a.getBoundingClientRect(); return { href: a.getAttribute('href'), cls: a.className, x: Math.round(r.left), y: Math.round(r.top) }; });
      await tv.waitForFunction(() => document.activeElement && document.activeElement.matches('#ytc-list a.t'), null, { timeout: 5000 });
      const a = await focused();
      await tv.keyboard.press('ArrowRight');
      const b = await focused();
      assert.ok(b.href !== a.href && b.x > a.x && Math.abs(b.y - a.y) < 4, `→ moves along the row: ${JSON.stringify([a, b])}`);
      await tv.keyboard.press('ArrowDown');
      const c = await focused();
      assert.ok(c.y > b.y + 40 && Math.abs(c.x - b.x) < 4, `↓ lands on the card below: ${JSON.stringify([b, c])}`);
      await tv.keyboard.press('ArrowUp');
      assert.equal((await focused()).href, b.href, '↑ comes back');
      await tv.keyboard.press('Enter');
      await tv.waitForFunction(() => location.pathname === '/watch', null, { timeout: 8000 }).catch(async e => { throw new Error(e.message + ' at ' + tv.url() + ' focus ' + JSON.stringify(await focused())); });
      assert.equal(new URL(tv.url()).pathname + new URL(tv.url()).search, b.href.replace(/&amp;/g, '&'));
      await tv.waitForFunction(() => document.activeElement && document.activeElement.id === 'ytc-player', null, { timeout: 8000 });
      await tv.waitForSelector('#ytc-player video');
      await tv.keyboard.press('ArrowDown');
      assert.ok(await tv.evaluate(() => !!document.activeElement.closest('#ytc-player .pctl')), '↓ from the player enters the control bar');
      await tv.keyboard.press('ArrowUp');
      assert.equal(await tv.evaluate(() => document.activeElement.id), 'ytc-player', '↑ from the bar returns to the player');
      assert.deepEqual(tvErrors, []);
    } finally {
      await tv.evaluate(() => localStorage.removeItem('ytc:platform')).catch(() => {});
      await tv.close();
    }
  });
  await step('TV mode: focus resting on a card prefetches its first media and warms googlevideo through a request (the shell\'s connection, not a <link rel=preconnect>); OK uses the prefetch', async () => {
    const tv = await ctx.newPage();
    const w0 = log.warm.length;
    try {
      await tv.addInitScript(() => { try { localStorage.setItem('ytc:platform', 'tv'); } catch {} });
      await tv.goto('https://www.youtube.com/results?search_query=tv+focus+prefetch', { waitUntil: 'commit' });
      await tv.waitForSelector('#ytc-list .c a.t');
      await tv.waitForFunction(() => document.activeElement && document.activeElement.matches('#ytc-list a.t'), null, { timeout: 5000 });
      await tv.waitForTimeout(1500); // a fresh tab: the config refresh (visitor data) lands first
      const g0 = log.gv.length; // this card may have played in an earlier step
      await tv.keyboard.press('ArrowRight');
      const href = await tv.evaluate(() => document.activeElement.getAttribute('href'));
      const v = new URL(href, 'https://www.youtube.com').searchParams.get('v');
      const mine = () => log.gv.slice(g0).filter(g => g.v === v);
      for (let i = 0; i < 40 && mine().length < 2; i++) await tv.waitForTimeout(50);
      assert.equal(mine().filter(g => g.a === 0).length, 2, 'video + audio first media while focus rests: ' + JSON.stringify(mine()));
      assert.ok(log.warm.slice(w0).includes('rr1---sn-fixture.googlevideo.com'), 'warmed with /generate_204: ' + log.warm.slice(w0));
      assert.equal(await tv.evaluate(() => document.querySelectorAll('link[rel=preconnect][href*="googlevideo"]').length), 0, 'no <link rel=preconnect> on TV');
      await tv.keyboard.press('Enter');
      await tv.waitForFunction(() => document.documentElement.dataset.ytcFirstFrame, null, { timeout: 8000 });
      assert.ok(!mine().slice(2).some(g => g.a === 0), 'the prefetched first requests were used: ' + JSON.stringify(mine().map(g => [g.itag, g.a, g.b, g.gen])));
    } finally {
      await tv.evaluate(() => localStorage.removeItem('ytc:platform')).catch(() => {});
      await tv.close();
    }
  });

  await step('TV panels fit the screen at 960×540 and 1920×1080 inside the overscan-safe area: Home\'s sign-in nudge, the sign-in panel (code + a long status, no code, signed in), the control bar, the player menus (and the remote walks the bar after one closed), skip button, captions', async () => {
    // The TV mode with a stand-in for the TV app's shell (localStorage ytc:platform and ytc:shell, the
    // development switches for what tv/shim/shim.js provides). localStorage e2e:tv picks its answers: 'code' = a sign-in code whose polls fail with a long unbroken reason, 'nocode' =
    // the code request fails, 'signed' = a signed-in account.
    const tv = await ctx.newPage();
    const tvErrors = [];
    tv.on('pageerror', e => tvErrors.push(String(e)));
    try {
      await tv.addInitScript(() => {
        const mode = localStorage.getItem('e2e:tv') || 'code';
        const why = 'https://oauth2.googleapis.com/device/token?' + 'x'.repeat(80);
        localStorage.setItem('ytc:platform', 'tv');
        localStorage.setItem('ytc:shell', JSON.stringify({
          auth_token: mode === 'signed' ? JSON.stringify({ token: 'ya29.' + 'a'.repeat(60), exp: Date.now() + 3600e3 }) : '{"token":null}',
          auth_start: mode === 'nocode' ? null : JSON.stringify({ user_code: 'WWW-WWW-WWW', verification_url: 'https://www.google.com/device', interval: 1, expires_in: 1800 }),
          auth_poll: JSON.stringify({ state: 'error', error: why }),
          auth_signout: '{}',
          error: 'HTTP 503 ' + why,
        }));
      });
      // Every box (elements and text lines) of `sel` inside its own content box (horizontally: a tall state
      // scrolls inside the panel), the focused control with its ring inside the panel's padding, the box
      // itself inside the overscan-safe area (5 % per side: 48 × 27 px at 960 × 540), nothing clipped.
      const fit = (sel, inner) => tv.evaluate(([sel, inner]) => {
        const el = document.querySelector(sel), W = innerWidth, H = innerHeight, cs = getComputedStyle(el);
        const r = el.getBoundingClientRect(), bad = [], e = 0.5, sx = W * 0.05, sy = H * 0.05;
        const px = v => Math.round(v);
        // A full-bleed bar (inner = 'bleed') keeps its content box in the safe area, a panel its whole box.
        const sr = inner === 'bleed' ? { left: r.left + parseFloat(cs.paddingLeft), top: r.top + parseFloat(cs.paddingTop), right: r.right - parseFloat(cs.paddingRight), bottom: r.bottom - parseFloat(cs.paddingBottom) } : r;
        if (sr.left < sx - e || sr.top < sy - e || sr.right > W - sx + e || sr.bottom > H - sy + e) bad.push(`${sel} ${[sr.left, sr.top, sr.right, sr.bottom].map(px)} outside the safe area of ${W}×${H}`);
        if (el.scrollWidth > el.clientWidth + 1 && cs.overflowX !== 'visible') bad.push(`${sel} scrolls sideways (${el.scrollWidth} > ${el.clientWidth})`);
        const pad = inner ? [parseFloat(cs.paddingLeft), parseFloat(cs.paddingRight)] : [0, 0];
        const out = (q, what, ring) => {
          // (In a full-bleed bar everything, rings included, stays inside the safe area.)
          const l = inner === 'bleed' ? sx : ring ? r.left : r.left + pad[0], rt = inner === 'bleed' ? W - sx : ring ? r.right : r.right - pad[1];
          if (q.width && q.height && (q.left < l - e || q.right > rt + e)) bad.push(`${what} ${[q.left, q.right].map(px)} outside ${[l, rt].map(px)}`);
        };
        const seen = { checkOpacity: true, checkVisibilityCSS: true }; // (opacity 0 anywhere up the tree = not shown)
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          if (n.nodeType === 3) {
            if (!n.data.trim() || !n.parentElement.checkVisibility(seen)) continue;
            const rg = document.createRange(); rg.selectNodeContents(n);
            for (const q of rg.getClientRects()) out(q, JSON.stringify(n.data.slice(0, 30)));
            continue;
          }
          if (!n.checkVisibility(seen)) continue;
          const ns = getComputedStyle(n), q = n.getBoundingClientRect(), what = n.tagName.toLowerCase() + '.' + n.className;
          if (n === document.activeElement && ns.outlineStyle !== 'none') {
            const o = parseFloat(ns.outlineWidth) + parseFloat(ns.outlineOffset);
            out({ left: q.left - o, right: q.right + o, width: q.width, height: q.height }, what + ' (focus ring)', true);
            // …and no ancestor that clips (overflow other than visible) cuts the ring.
            for (let p = n.parentElement; p && p !== el.parentElement; p = p.parentElement) {
              const ps = getComputedStyle(p), pr = p.getBoundingClientRect();
              if (ps.overflowX === 'visible' && ps.overflowY === 'visible') continue;
              if (q.left - o < pr.left - e || q.right + o > pr.right + e || q.top - o < pr.top - e || q.bottom + o > pr.bottom + e) bad.push(`${what}'s focus ring is cut by ${p.tagName.toLowerCase()}.${p.className}`);
            }
          } else out(q, what);
          if (n.scrollWidth > n.clientWidth + 1 && ns.overflowX !== 'visible') bad.push(`${what} clips its text (${n.scrollWidth} > ${n.clientWidth})`);
          if (n.scrollHeight > n.clientHeight + 1 && /hidden|clip/.test(ns.overflowY)) bad.push(`${what} cuts its content off (${n.scrollHeight} > ${n.clientHeight})`);
        }
        return bad;
      }, [sel, inner]);
      const shots = process.env.YTC_SHOTS;
      for (const [W, H] of [[960, 540], [1920, 1080]]) {
        await tv.setViewportSize({ width: W, height: H });
        for (const mode of ['signed', 'nocode', 'code']) {
          await tv.goto('https://www.youtube.com/', { waitUntil: 'commit' });
          await tv.evaluate(m => { localStorage.setItem('e2e:tv', m); sessionStorage.clear(); }, mode);
          await tv.reload({ waitUntil: 'commit' });
          await tv.waitForSelector('#ytc-acct .tva');
          if (mode !== 'signed') {
            // Home's sign-in nudge (signed out): the text and the focused button with its ring.
            await tv.waitForSelector('.hero [data-act=tv-signin]');
            await tv.focus('.hero [data-act=tv-signin]');
            await tv.waitForTimeout(250);
            if (shots && mode === 'code') await tv.screenshot({ path: path.join(shots, `tv-home-${W}.png`) });
            assert.deepEqual(await fit('#ytc .hero', false), [], `Home nudge at ${W}×${H}`);
          }
          await tv.focus('#ytc-acct .tva');
          await tv.keyboard.press('Enter');
          await tv.waitForSelector('#ytc-tvsign.on');
          if (mode === 'code') await tv.waitForSelector('#ytc-tvsign .tvc + .m ~ .tvst', { timeout: 5000 });
          if (mode === 'nocode') await tv.waitForSelector('#ytc-tvsign [data-act=tv-newcode]');
          if (mode === 'signed') await tv.waitForSelector('#ytc-tvsign [data-act=tv-signout]');
          await tv.waitForTimeout(400); // the panel's scale-in
          if (shots) await tv.screenshot({ path: path.join(shots, `tv-signin-${mode}-${W}.png`) });
          assert.deepEqual(await fit('#ytc-tvsign .tvp', true), [], `${mode} at ${W}×${H}`);
          assert.ok(await tv.evaluate(() => document.activeElement.closest('#ytc-tvsign') !== null), 'the focus is in the panel');
          await tv.focus('#ytc-tvsign [data-act=tv-close]');
          await tv.keyboard.press('Enter');
          await tv.waitForSelector('#ytc-tvsign:not(.on)', { state: 'attached' });
        }
        // The player's menus from the control bar (OK on a menu button), signed out (the last panel's shell said so).
        await tv.evaluate(() => localStorage.setItem('e2e:tv', 'code'));
        await tv.goto('https://www.youtube.com/results?search_query=lofi', { waitUntil: 'commit' });
        await tv.waitForFunction(() => document.activeElement && document.activeElement.matches('#ytc-list a.t'), null, { timeout: 5000 });
        await tv.focus('#ytc-list a.t[href^="/watch"]');
        await tv.keyboard.press('Enter');
        await tv.waitForFunction(() => document.activeElement && document.activeElement.id === 'ytc-player', null, { timeout: 8000 });
        await tv.waitForSelector('#ytc-player video');
        await tv.keyboard.press('ArrowDown');
        if (shots) await tv.screenshot({ path: path.join(shots, `tv-controls-${W}.png`) });
        assert.deepEqual(await fit('#ytc-player .pctl', 'bleed'), [], `control bar at ${W}×${H}`);
        for (const b of ['#ytc-speed', '#ytc-qual']) {
          await tv.focus(b);
          await tv.keyboard.press('Enter');
          await tv.waitForSelector('#ytc-player.menu #ytc-menu');
          await tv.waitForTimeout(300);
          if (shots) await tv.screenshot({ path: path.join(shots, `tv-menu-${b.slice(5)}-${W}.png`) });
          assert.deepEqual(await fit('#ytc-menu', false), [], `${b} menu at ${W}×${H}`);
          await tv.keyboard.press('Escape');
          await tv.waitForSelector('#ytc-player.menu', { state: 'detached', timeout: 3000 });
          // Closed (its content stays for the fade-out): the remote walks the bar again, ← leaves the shortcut.
          assert.equal(await tv.evaluate(() => document.activeElement.id), b.slice(1));
          const x0 = await tv.evaluate(() => document.activeElement.getBoundingClientRect().left);
          await tv.keyboard.press('ArrowLeft');
          assert.ok(await tv.evaluate(x0 => { const a = document.activeElement; return !!a.closest('.pctl') && a.getBoundingClientRect().left < x0; }, x0), `← moves along the bar after the ${b} menu closed`);
        }
        // The skip-sponsor button (shown here at rest: the fixture's segment passes in a moment) and the caption window.
        // (Captions may be off from earlier steps: c turns them on.)
        if (!(await tv.locator('#ytc-player.cctext').count())) { await tv.focus('#ytc-player'); await tv.keyboard.press('c'); }
        await tv.waitForSelector('#ytc-player.cctext .pcw', { timeout: 5000 });
        await tv.evaluate(() => document.querySelector('#ytc-player .psbt').classList.add('on'));
        await tv.waitForTimeout(500);
        if (shots) await tv.screenshot({ path: path.join(shots, `tv-skip-${W}.png`) });
        for (const sel of ['#ytc-player .psbt', '#ytc-player .pcw']) assert.deepEqual(await fit(sel, false), [], `${sel} at ${W}×${H}`);
      }
      assert.deepEqual(tvErrors, []);
    } finally {
      await tv.evaluate(() => { for (const k of ['e2e:tv', 'ytc:platform', 'ytc:shell']) localStorage.removeItem(k); }).catch(() => {});
      await tv.close();
    }
  });
  await step('TV app (signed in): the ready Home response and the hedge are the account\'s TVHTML5 Home request with the access token; a reload paints the ready response', async () => {
    const tv = await ctx.newPage();
    const token = 'ya29.' + 'h'.repeat(60);
    const wasRich = HOME.rich;
    HOME.rich = true; HOME.stamp = true;
    const homeReqs = () => log.api.filter(a => a.ep === 'browse' && a.body?.browseId === 'FEwhat_to_watch');
    try {
      await tv.addInitScript(t => {
        localStorage.setItem('ytc:platform', 'tv');
        localStorage.setItem('ytc:shell', JSON.stringify({ auth_token: JSON.stringify({ token: t, exp: Date.now() + 3600e3 }) }));
      }, token);
      // (The tab's first document may boot before the init script sets the TV flag: let it settle, then
      // start from an empty tab session.)
      await tv.goto('https://www.youtube.com/', { waitUntil: 'commit' });
      await tv.waitForTimeout(2500);
      await tv.evaluate(() => { sessionStorage.clear(); localStorage.removeItem('ytc-test-noready'); });
      const n0 = homeReqs().length;
      await tv.reload({ waitUntil: 'commit' });
      await tv.waitForFunction(() => /Fresh #/.test(document.querySelector('#ytc-list .c .t')?.textContent || ''), null, { timeout: 8000 });
      await tv.waitForFunction(() => { const r = sessionStorage.getItem('ytc:ready'); return r && JSON.parse(r).i.length > 30; }, null, { timeout: 8000 });
      const reqs = homeReqs().slice(n0);
      assert.ok(reqs.length >= 2, `the page and the ready response (${reqs.length})`);
      for (const r of reqs) {
        assert.equal(r.body.context.client.clientName, 'TVHTML5');
        assert.equal(r.headers.authorization, `Bearer ${token}`);
      }
      const title = await tv.evaluate(() => JSON.parse(sessionStorage.getItem('ytc:ready')).i[0].t);
      const n1 = homeReqs().length;
      await tv.reload({ waitUntil: 'commit' });
      await tv.waitForFunction(() => document.querySelector('#ytc-list .c[data-v] .t'), null, { timeout: 8000 });
      assert.equal(await tv.evaluate(() => document.documentElement.dataset.ytcHomeSrc), 'ready');
      assert.equal(await tv.evaluate(() => document.querySelector('#ytc-list .c[data-v] .t').textContent), title);
      assert.ok(homeReqs().slice(n1).every(a => a.t > Date.now() - 3000), 'no Home request before the paint');
      // A stalled TV Home request: the identical TVHTML5 request with the token at the deadline (after the
      // next ready response has landed, so the stall meets the page's own request).
      await tv.waitForFunction(() => { const r = sessionStorage.getItem('ytc:ready'); return r && JSON.parse(r).i.length > 30; }, null, { timeout: 8000 });
      await tv.evaluate(() => { localStorage.setItem('ytc-test-noready', '1'); sessionStorage.removeItem('ytc:ready'); sessionStorage.setItem('ytc:bt', '100,100,100,100,100'); });
      HOME.stall = 1;
      const n2 = homeReqs().length;
      await tv.reload({ waitUntil: 'commit' });
      await tv.waitForFunction(() => document.documentElement.dataset.ytcHedge === 'hedge' && document.querySelector('#ytc-list .c[data-v]'), null, { timeout: 5000 });
      const h = homeReqs().slice(n2);
      assert.equal(h.length, 2);
      assert.equal(JSON.stringify(h[0].body), JSON.stringify(h[1].body), 'identical bodies');
      assert.ok(h.every(r => r.headers.authorization === `Bearer ${token}` && r.body.context.client.clientName === 'TVHTML5'), 'both with the access token');
    } finally {
      HOME.stall = 0; HOME.stamp = false; HOME.rich = wasRich;
      await tv.evaluate(() => { for (const k of ['ytc:platform', 'ytc:shell', 'ytc-test-noready']) localStorage.removeItem(k); }).catch(() => {});
      await tv.close();
    }
  });

  await step('no live network traffic; no YouTube player JS or embed ever requested', async () => {
    assert.ok(log.external.every(u => /ytimg\.com|ggpht\.com|googleusercontent\.com/.test(new URL(u).hostname)), log.external.filter(u => !/ytimg|ggpht|googleusercontent/.test(u)).join(','));
    // (The DNR step fetches /s/player/abc/…/base.js itself to prove it is not blocked.)
    const playerJs = log.assets.filter(u => /\/s\/player\/|base\.js|www-player|widgetapi|iframe_api/.test(u) && !u.includes('/s/player/abc/'));
    assert.deepEqual(playerJs, []);
    assert.deepEqual(log.embeds, []);
  });
  await step('no player failure ever left by itself: every native watch page was opened by the test or the panel\'s Open on YouTube', async () => {
    const auto = log.docs.filter(u => /\/watch\?/.test(u) && u.includes('yt-native=1') && !nativeOk.includes(u));
    assert.deepEqual(auto, []);
    assert.ok(log.docs.some(u => u === 'https://www.youtube.com/watch?v=unplayable1&t=7s&yt-native=1') || ONLY, 'the manual button was exercised');
  });
  await step('no page errors', async () => { assert.deepEqual(errors, []); });

  await ctx.close();
  collector.close();
  dg.close();
  fs.rmSync(profile, { recursive: true, force: true });
  if (load !== EXT) fs.rmSync(load, { recursive: true, force: true });
  const failed = results.filter(r => r[0] === 'FAIL'), skipped = results.filter(r => r[0] === 'skip');
  console.log(`\n${results.length - failed.length - skipped.length}/${results.length - skipped.length} passed` + (skipped.length ? ` (${skipped.length} skipped: development build only)` : '') + ` · ${DEV_BUILD ? 'development' : 'public'} build`);
  process.exit(failed.length ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
