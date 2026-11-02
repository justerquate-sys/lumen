# YourTube

**Your own YouTube front-end, running at www.youtube.com.** YourTube is a browser extension that stops
YouTube's web app before it starts and renders its own fast, quiet pages from YouTube's own data (the
InnerTube API), with its own video player. Your account, subscriptions, history and playlists keep working,
because it is still youtube.com with your session. The logic is Rust compiled to WebAssembly; JavaScript is
only a small generated loader and a generic bridge.

> **Status: experimental.** It relies on unofficial YouTube interfaces that can change at any time. When
> something breaks, the player offers a way to YouTube's own page for that video (see "Escape hatch").

Works in Chromium browsers (Chrome, Brave, Edge, Vivaldi, Opera, Chromium) on macOS, Windows and Linux.
A Firefox version is planned.

## Screenshots

_To be added:_

- Home with the topic row and Filters
- Search results with the dropdown (history + suggestions)
- Watch page: player, chapters, description links, comments
- Player menus (Speed / Quality / CC) and the SponsorBlock bar
- Voice search listening pill

## Features

- **Fast pages.** Home, Subscriptions, History, Watch later, Playlists, Liked videos, Search, channels and
  playlists, rendered by Rust from InnerTube responses. Feeds preload up to 100 videos in idle time; hovering
  a link prefetches its page; back/forward and reloads repaint instantly from memory or this tab's session.
- **Its own video player** (no YouTube player JavaScript, no iframe): VP9/AV1/H.264 via Media Source
  Extensions, quality and codec choice, speed 0.25–2.5×, chapters with storyboard frames on the seek bar,
  captions you can drag and resize, live streams with DVR, resume positions, watch history kept in your
  account. Failures stay in the player with the reason and a **Try again** / **Open on YouTube** choice.
- **SponsorBlock skipping** built into the player, privacy-preserving (see "Privacy").
- **No Shorts unless you want them**, no ads, no Premium/membership upsells. Optional filters: minimum length, Music, Live, popularity order, playlists only.
- **Your own Home topics** next to YouTube's own chips (rename, reorder, hide them).
- **Block websites**: hide videos whose description links to a domain you blocked.
- **Search** with your recent searches and YouTube's suggestions in one dropdown; YouTube's own search filters.
- **Watch-page actions**: like/dislike, subscribe, save to playlists, share, comments with replies, the
  notification bell and the account menu, all optimistic (instant, rolled back with the reason on failure).
- **Voice search** (optional, your own Deepgram key): hover the mic or hold ⌥ Option / Alt and speak.
- **Keyboard first**: `?` lists the shortcuts; `/` focuses search; `g h` Home, `g s` Subscriptions…
- **Motion that never costs speed**: every result is usable on its first frame; animation rides on top
  ([docs/motion.md](docs/motion.md)).
- Charcoal + mint theme.

## Install (no build needed)

The repository contains the ready-to-load extension in [`extension/`](extension/).

1. Get the code: **Code → Download ZIP** on GitHub and unzip it, or `git clone` the repository. Keep the folder
   somewhere permanent: the browser loads the extension from it every time it starts.
2. Open your browser's extensions page:

   | Browser | Address |
   |---|---|
   | Chrome / Chromium | `chrome://extensions` |
   | Brave | `brave://extensions` |
   | Edge | `edge://extensions` |
   | Vivaldi | `vivaldi://extensions` |
   | Opera | `opera://extensions` |

3. Turn on **Developer mode** (a switch at the top right; in Edge it is in the left sidebar).
4. Click **Load unpacked** and choose the **`extension`** folder inside the repository (the folder that
   contains `manifest.json`), not the repository folder itself.
5. Open https://www.youtube.com/. Sign in to YouTube as usual if you want your feeds.

This is the same on macOS, Windows and Linux. To update: replace the folder's contents with the new version
(or `git pull`), then press the extension's reload button (↻) on the extensions page and reload YouTube tabs.
To remove it: **Remove** on the extensions page.

If you use another extension that changes youtube.com (a restyler, an ad blocker's YouTube rules), turn it
off for a first try: two extensions acting on the same page can get in each other's way.

### Optional: voice search key

Voice search uses [Deepgram](https://deepgram.com/)'s speech recognition with **your own** API key.
Right-click the mic in the search box (or focus it and press the context
menu key), paste the key, **Save**. The browser asks once for microphone access on youtube.com. Without a key
the mic only offers this panel; nothing is sent anywhere.

## Escape hatch

- The player's failure panel has **Open on YouTube**, which opens YouTube's own page for that video
  (`?yt-native=1`). Leaving that video brings you back to YouTube's client pages.
- Account pages, sign-in, Premium, live chat and embeds always stay YouTube's own.
- If the extension ever fails to start on a page, that page falls back to YouTube's own site.

## Privacy

YourTube has no server, no analytics and no account of its own. What it stores stays in your browser's
extension storage on this device (preferences, the last 100 searches, resume positions for up to 2,000
videos, your topics and blocked websites, an optional Deepgram key). Feeds, descriptions and recommendations
are kept in memory and in this tab's session only.

Every outside service it talks to, and when:

| Service | When | What it sends |
|---|---|---|
| **YouTube** (`www.youtube.com`, `*.googlevideo.com`, `i.ytimg.com`, `yt3.ggpht.com`) | Whenever you use it, like youtube.com itself | The same kind of requests YouTube's own site makes, with your YouTube session if you are signed in. Video streams come from googlevideo. YouTube's own logging/ad endpoints (`log_event`, `/pagead/`, `csi_204`, doubleclick) are blocked. |
| **SponsorBlock** (`sponsor.ajay.app`) | When a video starts, unless you switch SponsorBlock off in the player | Only the first 4 hex characters of the SHA-256 hash of the video id (k-anonymity: the answer covers ~150 videos and yours is picked locally). No cookies, no referrer, nothing reported back. Switched off, nothing is sent. |
| **Deepgram** (`api.deepgram.com`) | Only while you are dictating a voice search, and only if you saved a key | Your microphone audio (16 kHz mono) for that dictation and your key. The key never enters the YouTube page; the extension's background worker adds it. |

The extension's permissions: `storage`, `declarativeNetRequest` (the static rules in `extension/rules.json`)
and host access to exactly the sites above. The public build contains no telemetry and sends nothing
anywhere else; the development build (below) can report incidents and timings to a collector on your own
computer only.

## Known limits

- Experimental and unofficial: the player uses one unofficial InnerTube client profile (`VISIONOS`), which
  YouTube can change or block at any time. Some networks get YouTube's "confirm you're not a bot" check; the
  player then retries with your signed-in session or a fresh anonymous visitor, and shows the reason if all fail.
- Signed-in feeds were written against current formats but are mostly tested with signed-out data;
  brand accounts and other `authuser` indexes are not supported yet.
- Not built: writing comments, rating comments, the subscription bell levels, channel tabs other than Videos,
  live chat and live captions, Shorts-style looping.
- Age-restricted and members-only videos show the panel; **Open on YouTube** plays them there.
- Chromium browsers only for now.

More: [docs/architecture.md](docs/architecture.md) ("Known gaps").

## Build from source

You only need this to change the code. Requirements:

- [Rust via rustup](https://rustup.rs/): the pinned toolchain (1.99.1 + the `wasm32-unknown-unknown` target)
  installs itself from `rust-toolchain.toml` on first use.
- `wasm-bindgen-cli` 0.2.130: `cargo install wasm-bindgen-cli --version 0.2.130 --locked`
- Node.js 22 or newer.
- A POSIX shell for `build.sh` (macOS/Linux terminal; on Windows Git Bash or WSL).

```sh
./build.sh            # public build → extension/content.js, background.js, manifest.json
./build.sh --dev      # development build (see below) into the same folder
npm install           # once: Playwright for the browser tests
npx playwright install chromium   # once: the Chromium the tests drive
npm test              # cargo test --locked + the end-to-end browser test (no network)
npm run check:public  # extension/ holds the public build (run before committing)
```

`npm test` drives Playwright's Chromium; set `YOURTUBE_EXTENSION_CHROMIUM` to another Chrome/Chromium binary
that can load unpacked extensions to use that instead. `YTC_ONLY=<regexp> npm run test:e2e` runs matching
steps only.

### Public and development builds

`./build.sh` makes the **public build**: what is committed in `extension/` and what users load. `./build.sh
--dev` makes a **development build** in the same folder, which adds, at build time:

- the local debug-center collector: the host permission `http://127.0.0.1:47321/*`, incident and speed-telemetry
  delivery from the background worker to [`tools/debug-center`](tools/debug-center/) (Cargo feature `dev` for the
  Rust telemetry, `/* @dev-start */ … /* @dev-end */` blocks in `browser/*.js`);
- auto-reload: open YouTube tabs ask the worker every 4 s whether the files changed (`build-id.txt`), and the
  extension reloads itself after each build;
- the name "YourTube Client (dev)" and a build id in the version name.

Run `./build.sh` again before committing; `npm run check:public` fails on a development build.

## Repository layout

| Path | What |
|---|---|
| `extension/` | The unpacked extension, ready to load: `manifest.json`, `content.js` (generated), `background.js` (generated), `client.css`, `native.css`, `rules.json`. |
| `src/` | The Rust crate (`ytc-client`): routing, InnerTube requests and parsing, rendering, the player, features. |
| `browser/` | Browser-side sources: `bridge.js` (generic DOM/media primitives for Rust), `background.js` (service worker template), `manifest.json` (manifest template), `build.cjs` (bundler). |
| `build.sh` | Builds `extension/` from `src/` and `browser/`. |
| `tests/` | `e2e.cjs` (Playwright, all traffic stubbed), recorded InnerTube fixtures, media generators, a few `live-*.cjs` scripts that hit real services (run by hand). |
| `tools/debug-center/` | Optional local collector and speed dashboard for development builds. |
| `docs/` | [architecture.md](docs/architecture.md) (how everything works), [motion.md](docs/motion.md) (the motion law), [debug-center.md](docs/debug-center.md). |

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first: logic goes in
Rust, privacy rules are strict, motion follows [docs/motion.md](docs/motion.md), and every change keeps
`npm test` green with `extension/` holding the public build.

## License

[MIT](LICENSE) © 2026 justerquate-sys. YourTube is not affiliated with YouTube or Google.
