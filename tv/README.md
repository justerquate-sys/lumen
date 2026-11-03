# Lumen for TV (Google TV / Android TV)

The client on a TV, as the app **Lumen**, driven by the remote, signed in to the same YouTube account as your
other devices, updating itself from GitHub Releases.

It is the **same client** as the browser extension (`src/`: Rust → Wasm, `browser/bridge.js`,
`extension/client.css`), not a fork. The TV app is a thin [Tauri v2](https://v2.tauri.app) shell that opens
`https://www.youtube.com` in an Android WebView and injects the client at document start, the way
the extension's content script does. The client switches to its TV mode from a platform flag the
shell sets (`YTC_PLATFORM = "tv"`).

## Install it on your TV (first time)

(The same steps are in the main [README](../README.md#google-tv--android-tv).) The newest APK is always at
**`is.gd/lumentv`** →
https://github.com/justerquate-sys/lumen/releases/latest/download/lumen-tv.apk (every release uploads it as `lumen-tv.apk` besides `lumen-tv-<version>.apk` and is
marked *latest*). After this first install the app keeps itself up to date (see *Updates*).

**Option A: the Downloader app (no computer needed)**

1. On the TV: *Settings → System → About* → select *Android TV OS build* (or *Build*) seven times →
   "You are now a developer".
2. Install **Downloader** (by AFTVnews) from the Play Store on the TV.
3. *Settings → Apps → Security & restrictions → Unknown sources* (on Google TV: *Settings → Apps →
   Install unknown apps*) → allow **Downloader**.
4. In the Downloader app, enter: **`is.gd/lumentv`**, then install.
5. Find **Lumen** in *Apps* (it shows its banner); move it to your favourites if you like.

**Option B: ADB over Wi-Fi (from your computer)**

1. Enable developer options (step 1 above), then *Developer options → USB debugging* (on newer
   Google TV: *Wireless debugging*).
2. Note the TV's IP address (*Settings → Network*).
3. On the computer (Android platform-tools): `adb connect <tv-ip>:5555` (with *Wireless debugging*:
   `adb pair <ip>:<pair-port>` with the shown code, then `adb connect <ip>:<port>`), accept the
   prompt on the TV.
4. Download the APK (the link above) and `adb install lumen-tv.apk`

**Sign in:** open the app, choose *Sign in* (the rail's last item) or *Sign in with your phone* on Home, then
scan the QR code with your phone, or go to the address shown (google.com/device or youtube.com/activate), enter
the code shown on the TV and pick your account. That is YouTube's own TV sign-in: your history, subscriptions,
Watch later and playlists are the same as everywhere else. *Account → Sign out* revokes the TV's access.

**Remote:** the arrows move the focus (the focused card grows and gets a mint ring), OK selects, Back closes what
is open, then goes to the rail, then back (on Home it leaves the app). ← from the first column opens the rail
(Search, Home, Subscriptions, History, Watch later, Playlists, Liked, Account). *Search* opens the search screen:
OK on the field opens the TV keyboard (its microphone dictates), ↓ walks recent searches and suggestions. A video
plays full screen at once; any key shows the controls (they hide 4 s after the last key while playing): ←/→ on the
bar seek 10 s (faster while held, with the storyboard frame), OK plays/pauses, ↓ goes to the buttons (captions,
speed, quality, like, Watch later, more info) and once more to Up next. Media keys (play/pause, rewind,
fast-forward) work everywhere on the watch page. The look is documented in [design/](design/README.md).

## Updates

Everything is in the app: **About & updates**, the last item of the rail (under Sign in / Account, so it is there
signed out too). It shows the app version, the interface version and build, when the last check ran and what it
found, with one action focused:

- **Check for updates** asks GitHub now (the shell's `shell_check`).
- **Update now** (a new interface was found): it is already downloaded and verified; the screen restarts with it at
  once, no relaunch, still signed in.
- **Install update** (a new app was found): downloads it with its progress, checks its hash against the signed
  manifest and opens Android's installer (`shell_install`; the first time Android asks to allow installs from
  Lumen: allow it and press Back, the install goes on). Choose *Update*, then *Open*. Downloader is only ever
  needed for the very first install.
- **Retry** after a failed check, with the reason (no connection, GitHub's rate limit, a signature that does not
  verify).

The automatic checks stay: at launch and every 6 hours the app reads the repository's releases, takes the newest
one carrying `lumen-tv-manifest.json`, checks its ed25519 signature against the public key compiled into the app
(`tv/keys/release.pub`) and the SHA-256 of every file, and caches it. When one finds something, a small *Update
ready* notice appears at the top right of the pages (never over a video; the rail item gets a mint dot) and its
button opens About & updates. A new interface also goes live by itself the next time the app comes back to the
foreground (never mid-video). Offline, the app runs what it has; a broken or tampered download is ignored.

An app from before this screen (0.1.0 builds of 2 Nov 2026) cannot install from it: it offers a new app in its own
dialog once (*Install*); after that update everything is one click here.

Testing the flow on the emulator: a debug build reads `files/dev.json`; `node tv/scripts/release.cjs --dry-run
--no-build` signs `tv/out/release/` (`LUMEN_TV_KEY=<test key>`, `--apk <file>`, `SEQ`, `APK_VERSION`), `node
tv/scripts/release.cjs --serve 8775` serves it as a fake GitHub, and
`echo '{"release_api":"http://10.0.2.2:8775/repos/o/r/releases"}' | adb shell "run-as app.lumen.tv sh -c 'cat > files/dev.json'"`
points the app at it. (A throwaway key needs a test build with its public key in `tv/keys/release.pub`; never
commit that.)

## Build

Toolchain (all in `$HOME`, no Homebrew or sudo; `tv/scripts/env.sh` points at it and honours `JAVA_HOME`,
`ANDROID_HOME` and `NDK_HOME` if you keep them elsewhere): Temurin JDK 17, Android SDK cmdline-tools,
platform-tools, platform 36, build-tools 35, NDK 28.2, rustup targets
`aarch64-linux-android armv7-linux-androideabi` (for the shell's toolchain; the client keeps its own pinned one
from `rust-toolchain.toml`), `cargo install tauri-cli --version ^2`, plus what `./build.sh` needs
(wasm-bindgen-cli 0.2.130, Node 22+).

```sh
tv/scripts/build.sh              # debug APK (arm64) → tv/out/lumen-tv-<v>-debug.apk
tv/scripts/build.sh --release    # release APK (arm64 + armv7, signed) → tv/out/lumen-tv-<v>.apk
tv/scripts/build.sh --no-client  # reuse the last client bundle (shell-only changes)
tv/scripts/run.sh                # debug build + install + launch on the attached TV/emulator
(cd tv/core && cargo test --locked)   # the shell's logic: updates, sign-in, rules, storage
```

The first step is `./build.sh --tv`: the shared client built as the **public** build into
`tv/shell/src-tauri/bundle/` (`content.js`, the extension's `client.css`, `native.css` and `rules.json`, a
bundle sequence `seq` and the client version `client`; `browser/build.cjs --target=tv`). That folder is the
APK's built-in client and what a release publishes; `extension/` is not touched. `SEQ=<n>` sets the
sequence (default: the current Unix time; the app installs a bundle only when its `seq` is higher than the
one it runs).

In a browser, the TV mode can be tried without the shell: `localStorage['ytc:platform'] = 'tv'` on
youtube.com switches the client to it, and `localStorage['ytc:shell']` (JSON `{ "<command>": "<answer>" }`)
answers its shell calls (`tests/e2e.cjs` uses both).

### Keys (never in the repository)

All signing material lives in `~/.config/lumen-tv/` on the release machine; `.gitignore` refuses key and
keystore files anywhere in the tree. Only the public key `tv/keys/release.pub` is committed (the app compiles
it in).

| File | What | Lost? |
|---|---|---|
| `apk-release.jks` + `keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`) | signs the release APK (`gen/android/app/build.gradle.kts` reads the properties file) | Android refuses to update an installed app with an APK signed by another key: every TV must uninstall (losing its sign-in) and install by hand. |
| `release-key.pem` (ed25519, PKCS#8) | signs `lumen-tv-manifest.json` of every release | TVs never accept another client update until an APK with a new `release.pub` is installed by hand. |

**Back up both** (e.g. in a password manager or an encrypted drive, not in a cloud folder next to the
repository). Creating them the first time:

```sh
mkdir -p ~/.config/lumen-tv && cd ~/.config/lumen-tv
openssl genpkey -algorithm ed25519 -out release-key.pem
# the public key for tv/keys/release.pub (raw 32 bytes, base64):
openssl pkey -in release-key.pem -pubout -outform DER | tail -c 32 | base64
keytool -genkeypair -keystore apk-release.jks -alias lumen-tv -keyalg RSA -keysize 4096 -validity 36500
printf 'storeFile=%s\nstorePassword=…\nkeyAlias=lumen-tv\nkeyPassword=…\n' "$HOME/.config/lumen-tv/apk-release.jks" > keystore.properties
chmod 600 *
```

## Release

```sh
GITHUB_TOKEN=… node tv/scripts/release.cjs            # build, sign, publish
node tv/scripts/release.cjs --dry-run                 # build and sign into tv/out/release/ only
```

It builds the release APK (`tv/scripts/build.sh --release`), signs `lumen-tv-manifest.json` (the bundle's files and
the APK with their SHA-256, the bundle sequence, the client version) with
`~/.config/lumen-tv/release-key.pem` (`LUMEN_TV_KEY` overrides), verifies the signature with
`tv/keys/release.pub` (a mismatched key would publish a release no TV accepts) and publishes a GitHub release
`lumen-tv-<shell version>-<seq>`, marked *latest*, carrying the bundle, the APK twice (`lumen-tv-<version>.apk`,
which the signed manifest names, and `lumen-tv.apk` for the first-install link
`…/releases/latest/download/lumen-tv.apk`), `lumen-tv-manifest.json` and `lumen-tv-manifest.json.sig`.
`GITHUB_TOKEN` needs *contents: write* on the repository. GitHub's *latest* is the newest non-prerelease
release of the whole repository: if desktop releases are published there later, they must either come before
the next TV release or also carry `lumen-tv.apk`, or the first-install link answers 404.

The repository the app reads its releases from is compiled in: `LUMEN_TV_REPO=owner/name` when building (default
`justerquate-sys/lumen`); give `release.cjs` the same `LUMEN_TV_REPO` when publishing from a fork. A client-only
release (most of them) needs no new APK: the TVs pick up the bundle. For a new APK, bump `version` in
`tv/shell/src-tauri/Cargo.toml` (the version code is derived from it) and release.

Testing an update locally: `node tv/scripts/release.cjs --dry-run --no-build` then
`node tv/scripts/release.cjs --serve 8775`, and in a **debug** build point the app at it:
`echo '{"release_api":"http://10.0.2.2:8775/repos/o/r/releases"}' | adb shell "run-as app.lumen.tv sh -c 'cat > files/dev.json'"`
(`"oauth": "http://10.0.2.2:<port>"` points the sign-in at a mock the same way). Release builds
ignore `dev.json`.

## Architecture

```
tv/
  shell/src-tauri/        the Tauri app (Rust)
    src/lib.rs            window on https://www.youtube.com, the page's one command `ytc`
    src/shell.rs          state: account, storage, bundle in use, update thread
    src/host.rs           JNI: Kotlin → Rust (init, script, user agent, request rules) and
                          Rust → Kotlin (Keystore seal/open, bundle swap, update dialog, installer)
    src/script.rs         the document-start script: shim + client bundle; the desktop user agent
    gen/android/…/app/lumen/tv/MainActivity.kt, Host.kt
                          what Android forces into Kotlin: activity hooks (Back first to the client,
                          media keys, swappable document-start script), Keystore AES-GCM, the
                          WebViewClient wrapper applying the rules, the update dialog and installer
  core/                   the shell's logic without Tauri/Android, tested on the host:
    release.rs            signed manifest, verified cache, GitHub release lookup, APK download
    auth.rs               OAuth device flow of YouTube's TV client, refresh, revoke
    rules.rs              extension/rules.json (declarativeNetRequest subset) for the WebView
    store.rs              chrome.storage.local as a JSON file
  shim/shim.js            the extension runtime for the page: YTC_PLATFORM/YTC_SHELL, chrome.storage
                          and chrome.runtime, the stylesheet, keyboard-on-OK for text fields
  keys/release.pub        the release public key compiled into the app
  scripts/                env.sh, build.sh, run.sh, release.cjs, keys.sh/cdp.cjs (debug helpers)
```

In the shared client (`src/`): `tv.rs` (pure TV logic: spatial navigation `pick`,
remote keys, token and device-code parsing, TVHTML5 request rewriting; unit-tested) and
`app/tvglue.rs` (wiring: D-pad focus moves with the row anchoring and focus memory, the rail, the search
screen, the player overlay keys and scrubbing, More info, Back, the sign-in screen, the token from the shell,
focus that is never lost), `qr.rs` (the sign-in QR code), plus small hooks listed in `PORTING.md`; the TV styles
are the block `/* ===== TV app` at the end of `extension/client.css` (everything under `html[data-ytc-tv]`; one
layout at every resolution, rem = 1/60 of the width). Design reference: `tv/design/` (look sheet + tokens).

**What replaces the extension's parts on TV**

| Extension | TV app |
|---|---|
| content script at document_start (+ `client.css`/`native.css`) | `WebViewCompat.addDocumentStartJavaScript` for `https://www.youtube.com`, shim + bundle |
| `chrome.storage.local` | shell command → `files/storage.json` |
| declarativeNetRequest (`rules.json`) | `Interceptor.shouldInterceptRequest` → `rules.rs` (block; googlevideo fetched natively with `Access-Control-Allow-Origin: *`) |
| background.js: incidents, dev reload, voice socket relay | off on TV (`chrome.runtime` is inert; no mic) |
| browser cookies (Google sign-in) | YouTube TV device sign-in; `TVHTML5` + `Authorization: Bearer` for account calls |

**Account calls on TV.** Home, Subscriptions, History, Library, the account's playlists (`VL…`),
their continuations, the watch page's `player` call (so history pings carry the account) and actions
(like, subscribe, Watch later) go out as `TVHTML5` with the access token; search, channels, comments
and the streams (VISIONOS) stay anonymous WEB calls as on desktop. History pings
(`/api/stats/playback`, `watchtime`) carry the bearer and report `c=TVHTML5&cplatform=TV`.

## Risks and limits

- Whether YouTube honours every account call made with the TV token (feeds, like/subscribe, the
  watch history pings) can only be confirmed with a real sign-in; the flow up to the token is
  verified against Google (code issued, polling answers `authorization_pending`) and the exchange
  against a mock. If an account surface misbehaves, the fix is in `tv.rs`/`tvglue.rs`
  (`account_browse`, `tv_account_req`) and the TV parser in `parse.rs`.
- The client ID/secret are YouTube's public TV values (`tv/core/src/auth.rs`); if Google rotates
  them, update them there (new APK).
- googlevideo is fetched by the shell (Kotlin `HttpURLConnection`) to add the CORS header the
  extension's rule adds; on very slow TVs this costs some throughput. The rule can be dropped from
  `rules.json` (a bundle update) if it ever hurts.
- Android TV WebView versions differ by device; the client needs a recent one (Chrome 120+; the
  emulator ran 143). Very old Android TV 8/9 sets with an outdated WebView may misbehave.
