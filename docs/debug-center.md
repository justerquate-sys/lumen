# Debug center and speed telemetry (development builds)

Both exist only in the development build (`./build.sh --dev`). The public build has neither: its background
worker keeps no incidents and sends nothing, and the Rust telemetry is compiled off (Cargo feature `dev`).

## Debug center

Every incident (a player failure `player-failed`, a stuck episode `player-stuck` and its `player-recovered` outcome, InnerTube error, Rust panic, boot failure) becomes a JSON report: reason, route, client version, timings, sign-in state, and for the player its notes, every request with status/bytes/time (hosts and ranges only, no signatures, IPs or cookies), the format ladder and the bad itags. The Rust app hands it to the background worker (`browser/background.js`), which keeps the last 100 in `chrome.storage.local` (`ytcIncidentsV1`) and delivers them to the local collector (the public build's worker has none of this).

The collector is `tools/debug-center` (Rust, standard library only). It listens on `127.0.0.1:47321` and appends one line per incident to its incidents file, which lives in your user data folder, never in the repository (macOS `~/Library/Application Support/Lumen/incidents.ndjson`, Linux `~/.local/share/lumen/incidents.ndjson`, Windows `%APPDATA%\Lumen\incidents.ndjson`). Open http://127.0.0.1:47321/ in the browser for the latest reports and http://127.0.0.1:47321/perf for speed telemetry (see "Speed telemetry"). Running it, and starting it at login: [tools/debug-center/README.md](../tools/debug-center/README.md). Undelivered reports are retried once a minute while YouTube is open. chrome.storage.local `ytcCollectorV1` may name another collector URL.

## Speed telemetry

Goal (2 Nov 2026): track every speed metric of starting a video and everything else, so it can be
inspected at any time to find what slows the client down. Code: `src/telemetry.rs` (buffer, gate,
spans), `src/app/telglue.rs` (app hooks), `src/player/own/tel.rs` (player hooks), the bridge's
`tel_*` primitives (observers, input timestamps, frame callbacks) and `browser/background.js` (queue).

**How it is sent.** Each event is one small JSON object `{m, v, at, …breakdown}`: `m` the metric, `v` its value
(ms unless noted), `at` epoch ms. Events collect in memory and go out as one batch (≤ 200 events) at most
once a minute, when 200 are waiting, or when the tab is hidden or left, through the existing incident
channel. The background worker keeps batches in their own queue (`ytcPerfQueueV1`, last 30, removed once
delivered), so they never push an incident out, stamps every report with the build (`"build": "0.8.0
(<build id>)"`), and the collector appends it to the incidents file like any report. A batch:

```json
{"build":"0.8.0 (b9c1ba6d02c4c684)","kind":"perf","schema":2,"reason":"speed events","at":1793000000000,
 "client":"0.8.0","client_version":"2.20251030.01.00","signed_in_cookie":false,"page":"19a4…",
 "seq":3,"dropped":0,"overhead_ms":1.2,"js_overhead_ms":0.7,
 "events":[{"m":"nav.watch","v":38.4,"at":1793000000123,"route":"watch","mode":"push","input":"pointer",
            "src":"cache","handler_ms":0.6,"insert_ms":22.1,"reqs":[…]}, …]}
```

`page` identifies the document (one tab's page load), `seq` the batch within it, `dropped` events lost to the
caps, `overhead_ms` the time spent in the telemetry code since the last batch and `js_overhead_ms` the
time the browser observers' callbacks took.

**What is measured** (`v` in ms unless noted; every event also carries `at`):

| Metric | `v` | Breakdown |
|---|---|---|
| `nav.<route>` (`home`, `subs`, `search`, `watch`, `channel`, `playlist`, `wl`, `liked`, `history`, `playlists`) | input → the frame after the page's content was written (first cards / the watch page) | `mode` push / pop / boot, `input` pointer / keyboard / history / auto (autoplay) / boot (from the document's navigation start), `src` memory / snapshot / cache (watch data) / prefetch (hover/intent page cache) / inflight (a prefetch still running) / network, `handler_ms` (input → app), `insert_ms` (input → DOM write), `wait_ms`, `parse_ms`, `render_ms`, `reqs` (each InnerTube request it waited for: `ep`, `ms`, `bytes`, `status`, `via`) |
| `nav.fresh` | input → the fresh page replacing a snapshot or cached page | `route`, `parse_render_ms` |
| `req.<endpoint>` (`browse`, `next`, `search`, `navigation/resolve_url`, `suggest`, `sw.js_data`, `player-visionos`, `player-web`, `timedtext`, `sponsorblock`, `visitor_id`) | request → answer | `status`, `bytes`, `via` (page / cont / prefetch / spec / bg / bg-cont / suggest / config / resolve), `nav` (the current navigation waited for it), `route`; VISIONOS also `route` (anonymous / account / fresh visitor) and `refresh` |
| `feed.more` | the end of the list reached → the next cards' frame (only when the user had to wait) | `route` |
| `video.start` | input → first frame | `vid`, `input`, `watch_ms` (input → watch page visible), `vision` {`src` cache / prefetch / fetch, `age_ms`, `ms`, `route`, `bot_checks`}, `caps_ms` (decodingInfo), `mse_open_ms`, `streams_ms` (input → streams chosen), `start_p` → `target_p` (equal since 3 Nov 2026: no low start rung), `gv` (the first googlevideo requests: `kind`, `itag`, `ms`, `bytes`, `status`, `mirror`, `at`), `refreshes`, `mirror`, `stuck`, `hidden_wait`, `mse_rebuilt`, `live`, `media_pre` {`v`, `a`: "hit" (a finished hover media prefetch was used) / "inflight" (one still loading was taken over) / ""}, `slow` {`vision_hedge` {`fired`, `won`}, `hedge_fired`, `hedge_won` (googlevideo mirror twins), `throttle_detected`, `path` (steps taken: `mirror`, `refresh`, `drop`, `back` = the cap lapsed)} |
| `video.throttle` | a googlevideo request judged throttled: how long it had run | `vid`, `kind`, `itag`, `why` (`ttfb` headers late / `rate` body too slow), `step` (`mirror` / `refresh` / `drop`), `kbps` (its body rate), `bw_kbps`, `p`, `started`, `mirror` (≤ 30 per video) |
| `video.fail` / `video.abandon` | input → the player gave up / was left before a first frame | as `video.start`, plus `reason` |
| `video.target`, `video.1080` | input → the target quality / ≥ 1080p on screen | `vid`, `p`, `start_p` |
| `gv.head`, `gv.seg`, `gv.live`, `gv.hls` | googlevideo request → answer (every index request; segments: the first 8 per video plus every slow > 1.5 s or failed one) | `vid`, `kind` v / a, `itag`, `host`, `mirror`, `bytes`, `status`, `kbps`, `started` |
| `video.seek` | seek input → the frame at the new position (or playing again when it had to wait) | `vid`, `input`, `waited`, `paused`, `at_s` |
| `video.switch` | quality decision → the new resolution on screen | `vid`, `from_p`, `to_p`, `why` (user / auto) |
| `video.stall` | stall duration | `vid`, `cause` (switch / network / underrun), `p`, `at_s`, `bw_kbps` |
| `video.drops` | dropped frames, % (per rung shown) | `vid`, `p`, `dropped`, `frames` |
| `video.session` | the player's life | `vid`, `started`, `failed`, `first_frame_ms`, `target_ms`, `hd_ms`, `refreshes`, `mirror`, `stuck`, `stalls`, `stall_ms`, `seeks`, `switches`, `bad_itags`, itags, `bw_kbps`, `live`, `slow` (as in `video.start`, for the whole session) |
| `ui.menu`, `ui.toast` | input → the frame showing the player's menu / toast | `menu`, `decide_ms` |
| `lag.loaf` (`lag.longtask` where Long Animation Frames are missing) | frame duration (≥ 50 ms) | `block`, `render`, `layout`, `scripts` (top 3: `ms`, `type`, `inv`, `fn`, `src` path, `forced` layout), `route` |
| `lag.event` | event → next paint for one interaction (its longest event, ≥ 40 ms) | `name`, `delay`, `proc`, `pres`, `target` (tag, id, `data-act`), `route` |
| `lag.cls` | cumulative layout shift of a page (score, not ms), one per page left | `route` |
| `res.storyboard` | storyboard image load | `bytes`, `cached`, `route` |

**Cost.** No per-frame bridge calls and no layout reads: a mark is one `performance.now()`; "visible" is one
`requestAnimationFrame` + task after the write (`tel_frame`); browser entries are buffered by the observers and
pulled once per batch. Volume caps: 200 events per batch, 400 kept while undelivered, 300 browser entries,
32 open spans, per video 40 seeks / 30 switches / 30 stalls / 8 sampled segments. Measured in the e2e
run: the e2e timing lines stay where they were (see the timing table) and a whole search → watch → seek →
leave session spends ~1–2 ms in Rust telemetry and < 1 ms in the observers.

**Privacy and the gate.** Video ids and route kinds only: no search terms, URLs, tokens, cookies, headers,
visitor data, descriptions or keys (the e2e test checks a batch for the SAPISID cookie, visitor data, the
search terms and googlevideo signatures). The gate is the build: telemetry exists only in the development
build (Cargo feature `dev`, the `@dev` blocks of `browser/*.js`, the `http://127.0.0.1:47321/*` host permission);
there chrome.storage.local `ytcTelemetryV1: false` turns it off. The public build records, stores and sends
nothing (its background worker also stores no incident).

**Reading it.** http://127.0.0.1:47321/perf: per metric count/p50/p90/p99/max (window: hour … everything,
version filter, metric filter such as `nav.*`), the p50 trend per build (▲/▼ for > 20 % moves), incidents
and errors per build (per 100 navigations, telemetry overhead per batch) and the 20 slowest events per
metric with their full breakdown. From the shell:

```sh
# every video start of the last 24 h, slowest first
curl -s 'http://127.0.0.1:47321/perf.json?metric=video.start&since=24h' | jq 'sort_by(-.v) | .[:5]'
# navigations of one build since an epoch-ms time (metric prefix with *)
curl -s 'http://127.0.0.1:47321/perf.json?metric=nav.*&since=1793000000000&version=0.8.0%20(b9c1ba6d02c4c684)' | jq 'group_by(.src) | map({src: .[0].src, n: length})'
```

`/perf.json` parameters: `metric` (exact, or a prefix ending in `*` or `.`; empty = all), `since` (epoch ms,
a duration ago in ms, or `30m` / `6h` / `7d`, `all`), `version` (the exact `build` string), `limit`
(newest rows, default 10000). Each row is the event plus `received`, `version` and `page`.
