# Lumen architecture and behaviour

The design reference for contributors: how the client works, page by page, and why. For installing and
building see the [README](../README.md); for the motion rules see [motion.md](motion.md); for the
development-only debug center and speed telemetry see [debug-center.md](debug-center.md).

Instead of modding YouTube's web app, the extension stops it at `document_start` and renders its own pages
from YouTube's InnerTube API. The logic is Rust compiled to Wasm (`src/`); JavaScript is only the generated
loader and a generic bridge (`browser/bridge.js`).

## Build pipeline

`build.sh` compiles the crate for `wasm32-unknown-unknown` (`--features dev` for the development build), runs
`wasm-bindgen` into `target/pkg/`, then `browser/build.cjs` writes the loadable extension:

- `extension/content.js`: `window.stop()`, the bridge, the wasm-bindgen glue and the Wasm bytes (base64 inside a
  block comment, read back with `Function.prototype.toString`), then `boot()` (or `boot_native()` on `?yt-native=1`
  pages). A boot failure falls back to YouTube's own page.
- `extension/background.js` and `extension/manifest.json` from `browser/background.js` and the manifest template
  `browser/manifest.json`. The public build drops every `/* @dev-start */ … /* @dev-end */` block; the development
  build keeps them and adds the debug-center host permission, a "(dev)" name and a build id (`version_name` and
  `build-id.txt`, which drive the auto-reload).
- `client.css`, `native.css` and `rules.json` are hand-written and live directly in `extension/`.

Browser targets: `browser/build.cjs` has a `TARGETS` table with one entry today, `chromium` → `extension/`.
A second browser (Firefox: `background.scripts` instead of a service worker, `browser_specific_settings`)
gets its own manifest template next to `browser/manifest.json` and its own output folder; the Rust crate,
the bridge and the CSS are shared.

## Escape hatch (native YouTube)

- The player's failure panel has an **Open on YouTube** button (there is no header link any more). It opens the
  video with `?yt-native=1`, which loads YouTube's own site for that page (the content script is excluded by glob and the DNR rules allow
  everything that document needs). Reloading without the parameter returns to the client.
- These paths always stay native: `/embed/*` (YouTube's embed; not used by the client, which refuses to play on youtube.com), `/live_chat*`, `/account*`, `/premium*`,
  `/paid_memberships*`, `/signin*`, `/logout*`, `/redirect*`.
- If the Wasm ever fails to boot, the loader falls back to `?yt-native=1` for that page.

### YouTube's own pages keep the client's setup (native mode cleanup)

Pages the client leaves to YouTube (the player panel's **Open on YouTube**, routes the client does not
render) get a slim version of the old extension's cleanup (2 Nov 2026), so no second extension is needed for them. Only these pages: the manifest's second
content script (`include_globs: *yt-native=1*`) injects `native.css` and boots `content.js` in native
mode (`boot_native`); client pages are untouched.

- **`native.css`** (hiding and theming, no JavaScript timing): the old `theme.css`, `watch-page.css`,
  `player-controls.css`, `captions.css` and `promotions.css` rules with their verified selectors, gated on
  `html[data-ytc-native]`. Header without voice search, Create or the locale label, the plain YouTube
  wordmark instead of the Premium logo; YouTube's guide trimmed to Home, Subscriptions, History, Playlists,
  Watch later, Liked videos (no Shorts, You/Your channel/Your videos/Downloads/courses/podcasts, channel
  list, Explore, More from YouTube, settings rows, footer); no Playables; Shorts shelves and cards out of
  feeds unless the client's Shorts switch (`shortsInFeeds`) is on (`/shorts/` itself untouched); Premium /
  membership banners and their feed slots hidden; watch page without Join, Thanks, the action overflow,
  Auto-dubbed badges, hashtags above the title, Summary / How this was made / Games / Transcript / repeated
  channel / Video details sections, and without the related chip row; player without the fullscreen More
  videos grid, quick actions, title overlay, channel watermark, gradients and cinematics, black around the
  picture, mint progress; the client's charcoal/mint palette over YouTube's theme variables.
- **`src/native/clean.rs`** (what needs logic; pure parts unit-tested): captions in the client's look
  and place (cyan Arial with a dark outline, no backgrounds, upper-left corner and size from the same
  `ytcConfigV1.p.cs` the client player uses, via CSS variables on `<html>`); drag a cue to move it, wheel
  over it or use the slim range beside it to size it, and the result is written back into `p.cs` (read,
  modify, write: other config fields are kept); the "click gear for settings" notice is hidden. CC on/off
  follows the client's `p.cc` in a video's first seconds (at most 3 clicks on YouTube's own CC button), and
  a deliberate toggle here (click or `c`) becomes the remembered choice. 0–9, `+`/`-`/`=`/`_` and `V` are
  swallowed while the page or player has the keyboard (not in text fields, dialogs or with modifiers); the
  fullscreen bare-video wheel never opens YouTube's More videos drawer (a passive window listener decides;
  only the player box can cancel a scroll). Offer dialogs (upsell / Premium spotlight / members-only) and
  offer mealbars are closed through their own dismiss button, never a trial button; account notices stay.
  "Auto-dubbed" badges are matched by text and marked for the stylesheet.
- **`bridge.js`**: generic page primitives for this (`root_attr`, `root_var`, `page_watch` = one
  MutationObserver coalesced to a frame, `page_attr`, `page_texts`, `page_mark`, `page_click`,
  `page_mount`, `page_value`, `page_guards`); every selector and decision comes from Rust.
- Leaving that native video for anything the client renders still reloads into the client (`native/mod.rs`).
- Size: `content.js` +26 KB (Wasm +10 KB, glue and bridge +12.5 KB); `native.css` 17 KB, loaded on native pages only.
- Left out (old extension features not carried over): the player hover menus (direct Speed/Quality/CC
  entries, compact menus, hidden settings rows), the seven-link pinned sidebar (YouTube's guide is trimmed
  instead), description resource-link panel and website blocking, Watch later card buttons, Music/Live/
  duration/order filters, comments/notifications/account-menu restyling, search page chip cleanup, history
  and resume records, separate speaker caption windows (every cue window takes the shared place).

## Use

Layout: a top bar with the YouTube logo (→ Home; **resting the pointer on it for 250 ms goes Home without a
click**, see below), a centered search box (≤ 720 px; `/` focuses it from
anywhere) and, on Home and Search, **Filters** left of the bell; a left sidebar with Home, History, Watch later, Subscriptions,
Playlists, Liked videos (no Shorts entry; Shorts appear only with the Filters **Shorts** switch). The sidebar is gone on watch pages,
where the player is as large as the window allows (16:9, up to the window height) and the top bar scrolls
away. Grid of five cards per row above 1700 px (4 / 3 / 2 / 1 as the window narrows).
**Motion law** ([motion.md](motion.md)): motion never delays anything. Content is final
and usable on the first frame; animation is a compositor-only layer on top (feedback on pointerdown,
interruptible transitions, ease-out or critically damped springs computed in Rust, no motion on keyboard
or back/forward). Skeletons have the final geometry (measured layout shift 0 on a cold search).

**Navigation motion** (`src/app/motionglue.rs`). *Watch open:* a pointer click on a card (or a playlist
row) reads the thumbnail's viewport rect (`bridge::click_rect`, one layout read before the swap), the page
is written exactly as before, and at the end of the same task `bridge::flip_from` paints `#ytc-player` from
that rect to its own box: one compositor-only `element.animate` (translate + scale, origin top-left),
easing = the critically damped spring from `motion.rs` over `--d-spring-soft` (458 ms). The player is final,
playing and clickable from the first frame (its poster is the card's thumbnail, so the picture grows into
place); `data-flip` lifts it above its neighbours while it moves. The skeleton areas that `next` fills
(description, playlist panel, Up next, related list; the whole info block when no card data was known)
settle in with a 180 ms opacity fade. No morph and no fades for keyboard activation, `g`+letter, back/forward
or restored pages (instant); no morph when the thumbnail is off-screen; reduced motion fades the player in
(200 ms) instead. Back from a watch page has no reverse morph: back/forward are instant by law, and the
source card is not on screen before the page swaps. *Theater:* a pointer click on the theater button morphs
the player from its old box to the new one (`--spring`, 351 ms); `t` is instant. Measured (e2e, cached
page): click → next frame 16.1 ms with the morph vs 16.8 ms without; → frame painted 24.2 vs 29.5 ms.
*Gate:* keyboard and back/forward paints set `#ytc[data-still="1"]` until their first frame has rendered
(`bridge::frame`), which turns `@starting-style` entrances off, so a restored page (with its comments)
never fades in. *Comments:* appended pages and replies settle in (opacity + 8 px × `--mv`, 220 ms);
Top/Newest switch the persistent tabs at once and keep the old list dimmed until the new first page
replaces it (the section never collapses under the reader). *Up next:* the countdown panel enters
(`@starting-style`) and runs out as a linear progress line; the Autoplay switch is toggled in place (its
knob transitions). *Sidebar:* hover wash instant in / eased out (`::before` opacity) and each icon's own small
gesture on the spring (Home lifts, History turns back, Watch later turns forward, Playlists nudges, Liked tips),
press scale .97 with the icon squeezing. The active pill is one persistent element (`#ytc-sdi`, a strip as tall as
the six links clipped to the active row by `--st`/`--sb`, registered `@property` lengths written by
`App::side_move`): a pointer navigation moves both edges on the house spring, the leading edge on `--d-spring`
and the trailing one on `--d-spring-soft` (`data-dir`), so it stretches toward the new page and settles while the
label turns mint; g+letter, back/forward and restored pages move it at once (`data-still`); on a page without a
link it fades where it is. *Header:* the logo tile swells and its play mark leans forward on hover, the whole
logo gives on press. **Zero-click Home** (2 Nov 2026: hovering the logo
activates it, no click needed; `app/logo.rs`): resting the pointer on the logo (tile or word) for 250 ms
(`logo::DWELL_MS`) opens Home; a mint pill behind the logo fills left to right over that time (a linear clip-path
wipe, like the player's Speed/Quality rows; reduced motion fades it). Home's first page is requested the moment
the pointer enters the logo, so at the end of the dwell Home paints from memory (no network wait). Passing over
never navigates: leaving the logo cancels (the pill retracts), coming back starts over, and after a dwell or a
press the pointer must leave before another starts. Nothing starts while a popover or menu is open (account,
notifications, Save, Share, Filters, the topic editor, the search dropdown, a player menu, the shortcut sheet, the
import dialog) or a text field has focus. A click is instant as before, the keyboard is unchanged. **Home on
Home** (click, dwell or `g h`) keeps the page: it jumps to the top, shows a held **New videos** feed, and fetches
a fresh feed only when the one shown is older than the background age (2 min; the same rule as returning to
Home from another page); no re-render and no extra history entry. With a Topic or YouTube chip showing it goes
back to the plain Home feed as before. The search field is one surface under the input and button (`.qf`: a border layer and a fill
layer, both clipped): pointer focus widens it 8 px each side (clip-path on the house spring, exact 21 px ends on
every frame), the button slides flush and turns mint and the magnifier glances once; the text box never moves and
the first keystroke paints on its keydown frame (e2e). `#ytc-search.now` (set on every key press, cleared on every
pointer press, i.e. before focus moves) makes keyboard focus and the blur after Enter instant. A clear control (×)
is there from the first character, empties the field without blurring it (`data-keep-focus`) and spins out. The
dropdown unfolds from the field (its surface grows from a 14 px strip, `--e-out` over `--d-panel`) and its rows
settle in 25 ms apart, clickable at once; typing, arrows, Escape and Enter change it with no motion. The bell tilts
on hover and rings once when the unread count rises; the badge is always in the DOM and grows in, pulses on a new
number (the number itself is final at once) and shrinks away when the list opens. The avatar leans in on hover and
an open account menu rings it in mint. A keyboard focus ring settles in from 4 px out (`ytcfocus`).

**Player and watch-page motion** (`src/player/own.rs`, `src/app/motionglue.rs`, client.css "Icon morphs";
pointer only, every state final first; keyboard changes are instant: a key sets `.kf` on the player for 450 ms).
*Icons change form:* one persistent icon per control, its SVG `d` paths interpolating on `--spring` when Rust flips the
state class on the media event: play's triangle halves become the pause bars, the theater frame widens/narrows, the
fullscreen corners fold inward; the volume waves leave toward the speaker (outer one below 50 %) and the mute cross
turns in; CC wipes its filled artwork in (clip-path, `--spring-pop` scale). *Center pulse:* a click on the video leaves
the action's icon growing and fading (560 ms keyframes, `bridge::replay`). *Seek bar:* a press swells a mint ripple
where it landed; the knob grows while held; the tooltip's surface morphs its width between chapter titles as you sweep
(`bridge::box_morph`), still centred 1:1 on the pointer; the control-bar chapter label's new title rises in. Volume
slider: mint fill (`--v`) and a thumb that grows on hover/press. *Menus:* switching Speed/Quality/CC (or the Audio tab,
or a codec) while the panel shows morphs the panel's surface (its `::before`, under a drop-shadow) and content box
with clip-paths from the old menu's height to the new one's on `--spring`, the new content sliding in from its
shortcut's side (160 ms); a sweep retargets from the on-screen size; content is final and clickable at once. A
choice's selected tint is one persistent mark per group (`bridge::mark_slide`) that springs to the new choice. Pointer
toasts pop in; LIVE breathes a ring at the edge. *Watch page:* a pointer open settles the title and channel row in
under the growing player, then the description's lines and the related cards as a short wave when `next` fills them;
description links draw their underline from the left (across wrapped lines); Block turns into "Blocked · Undo" growing
from its left edge. The chapter list's highlight and the playlist panel's current row are persistent marks that spring
to the new row as playback crosses a chapter or the playlist moves on (instant for keys, back/forward and first
placement); Top/Newest's mint pill travels over the tabs; a reopened reply thread unrolls from the top; Up next's Play
now fills with mint as the countdown runs (its label inverts exactly at the fill edge).

**Feed and chip motion** (`src/app/fxglue.rs`, client.css cards/chips/filters; pointer only, every
write final first). *Cards:* hover eases the picture (and a playing preview) 4.5 % closer inside its fixed
rounded frame on `--spring` over `--d-spring-soft`; the frame, duration badge and progress bar stay crisp;
a depth gradient (`.th::after` opacity) rises; the watched bar thickens (scaleY); the title turns mint;
press gives the frame .985 and settles the picture back. The hover preview fades in once it lands
(`fade_imgs`). *Entrance wave* (`bridge::stagger`, no layout read, ≤ 15 cards, 24 ms apart, ≤ 7 steps,
320 ms `--e-out`): when a pointer-initiated load replaces the skeleton, the text rises 8 px into place where
the bars were and the thumbnails fade (their grey boxes stay); a pointer-picked YouTube chip or topic
brings its first cards in whole (12 px). Never on the first page load, keyboard, a submitted search,
back/forward (`mo.calm`), restores, snapshot repaints, SWR refreshes or appended chunks. *Chip row and
Filters segments:* a sliding selection pill (`bridge::pill_slide`, "duplicate, clip, slide": a copy of
the row in its selected look, `.fx-pg`, is clipped to the old chip's rounded box and the clip slides and
morphs to the new chip on `--spring`, so the text inverts exactly at the pill edge; the real chip is final
and clickable at once; a chip feed that answers mid-slide re-renders the row and `pill_carry` continues the
slide; keyboard selections are instant). Removing/moving/hiding a chip reflows the row
(`bridge::flip_mark`/`flip_play`: survivors slide from their old place, a copy of the removed chip shrinks
out in 160 ms). *Panels:* Filters and + Topic morph out of their button (clip-path from a button-sized
rounded box + a 43 px drop on `--spring`, opacity `--d-pop`; back into the button in 110 ms). Switch knobs
stretch while held; the length slider has a mint fill (`--v`) and a thumb that grows on hover/press; the
Filters summary's new text slides in (`bridge::nudge`). *Watch later:* the bookmark fills bottom-up
(clip-path on a filled copy, soft spring) and a pointer save pops once (`--spring-pop`, the one
underdamped spring, from `motion::pop()`); removing drains it faster. The note under the bar and active
search-filter chips enter with `@starting-style` (gated by `data-still`).

**Voice search** (2 Nov 2026; `src/features/voice.rs` decides, `src/app/voiceglue.rs` wires it,
`background.js` relays the socket). A mic sits in the search field before the search button. *Hover:* resting the
pointer on it 200 ms starts dictation (leaving earlier does nothing); the glyph turns mint over exactly that dwell,
then the 32 px circle stretches leftwards into an 80×36 listening pill on the house spring, which is also the new,
bigger hover target (the button's box is the pill; at rest its clip-path, which clips hit-testing too, leaves only
the circle; the glyph never moves under the pointer). Five bars show the live level: the bridge writes `--lv`
(0..1, RMS −60…−10 dBFS) on the button at most every 33 ms and each bar is a `scaleY` of it with its own weight and
response time. The field shows the live transcript (finals in the text colour, interim words muted; a long one keeps
its end in view). Moving the pointer out of the pill ends dictation and searches; an empty dictation searches nothing
and the field returns to its text. A pointer click on the mic starts at once (or, once words were heard, searches).
*Keyboard:* hold **⌥ Option (Alt)** alone (≥ 250 ms, so Alt+← and Option+letter never start it) to talk, release to
search; any other key or a click while held cancels; Enter/Space on the focused mic toggles. Keyboard dictation shows
the pill at once (no morph). Escape cancels any dictation (nothing searched); navigation and hiding the tab stop it.
No sound effects. *Recognition:* Deepgram streaming, `nova-3`, `language=multi` (Russian + English code-switching),
`smart_format`, `interim_results`, `endpointing=300`, raw `linear16` 16 kHz mono (no container: the lowest latency
path) in 32 ms chunks (AudioContext at 16 kHz + a ScriptProcessor whose output is silent). Ending: if the last result
already settled the utterance (`speech_final`) the search runs at once; otherwise the mic runs 60 ms more (the tail
still in the audio pipeline), then `Finalize` + `CloseStream` and the search runs on Deepgram's flushed final (≤
1.5 s cap: then what was heard). A settled phrase warms its results page while you are still hovering. Trailing
sentence punctuation from smart_format is dropped from the query. *Where the socket lives:* in the background worker
(`chrome.runtime` port `ytc-sock`; audio chunks base64 over the port). YouTube's document CSP has no `connect-src`
(checked 2 Nov 2026), so the page could open it too, but then the key would have to enter the youtube.com renderer;
in the worker it never does. The worker sends the stored key only to `api.deepgram.com` over wss/https. *Key:* the
panel (right-click / menu key on the mic, or shown by itself when no key is set; grows out of the mic) takes the key
in a password field; `bridge::secret_save` stores it in `chrome.storage.local` `ytcVoiceKeyV1` without handing it to
Wasm and empties the field. Each state is one line: no key, saved, removed, rejected (the worker checks
`/v1/projects` with the key when a handshake is refused: 401/403 → "Deepgram rejected this key"), unreachable,
microphone blocked / missing. The microphone permission is the browser's own prompt for youtube.com (asked once).
The debug overlay shows `voice · key set|no key · phase · dictations · last: end → search ms`.

**Keyboard first** (`?` shows the sheet): `g h` Home, `g s` Subscriptions, `g y` History, `g w` Watch
later, `g p` Playlists, `g l` Liked; arrow keys walk the cards on feed pages (↑/↓ by the rendered row),
Enter opens; on watch pages the player keys below apply.

Pages: Home (`FEwhat_to_watch`; signed out or history off → a search-first page), Subscriptions,
History, Search (YouTube's recommendation shelves inside the results are dropped, first page and
continuations: "People also watched", "For you", "People also search for", "Previously watched", "From
related searches", … by title in English, Russian and German, and any video shelf that links nowhere; real
result shelves, e.g. a channel's linked "Latest from …", and channel/playlist results stay; the debug overlay
counts them as `recommendation shelves`), Watch (`/watch?v=…&t=…`, our own player, see below, title/channel/views/date/description/related),
Channel (`/@handle`, `/channel/UC…`, `/c/…`, `/user/…` → Videos tab), Playlist (`/playlist?list=…`,
including `WL`). Feeds, search, channel videos and related videos scroll infinitely. Shorts are hidden
unless the Filters **Shorts** switch is on (then Home, Search and related only); `/shorts/ID` opens as a normal watch page. The **description** is shown as the old extension showed it (
`src/description.rs`): one panel 12px below the channel/actions row, capped at 760px, fully open at natural
height, with views and date on top, then only the lines that carry a resource link (http(s)/mailto or a YouTube page,
redirects unwrapped to the real destination), each with its own text minus hashtags; a bare link takes the short line
above it as its label. Prose, hashtags and timestamp links are left out (chapters are listed separately).
Hovering a video link prefetches its watch data, so Watch opens with title, description and related
already loaded; hovering a channel, playlist or top-bar feed link prefetches that page too (kept 2 min,
memory only). Hovering a thumbnail for 0.5 s plays YouTube's own animated preview (`an_webp`, only when
the response lists one). The search box suggests as you type (YouTube's same-origin
`/complete/search`, shown as the box's native list; picking one searches). Back/forward restores pages
from memory instantly.

**Watch-page actions** (`src/actions/`): **Like / Dislike** (YouTube's own count label; click again
to remove), **Subscribe** beside the channel (unsubscribing needs a second click on "Unsubscribe?"),
**Share** (youtu.be link, optional "Start at" the current time, Copy), **Save** (a bookmark that is
filled in mint and reads "Saved" while the video is in any of your playlists: signed in, the membership is
asked once, quietly, 1.2 s after the watch page opened (`get_add_to_playlist`, kept in memory, the same answer
the chooser shows, so opening it sends nothing more; a failed quiet check stays quiet); ticking/unticking in
the chooser fills/empties it at once (optimistic, back on a rollback); the fill sweeps up on the soft spring,
keyboard changes are instant. Hover opens the
chooser with the player menus' intent/grace timing via `MenuCtl`; click pins it, Enter/Space opens it
from the keyboard, ↑/↓ move, Escape closes; hover never selects; checkboxes add/remove through
`browse/edit_playlist`; "New private playlist" creates one with the video). All actions are
**optimistic**: the control changes at once, one request per target is in flight, and a failure rolls
back to YouTube's last confirmed state with the reason beside the row (and an incident in the debug
center). Signed out, the controls ask you to sign in and send nothing.
**Chapters** (player-bar chapters, else the chapters panel, else the description's timecodes by YouTube's
rules: a timestamp at the start of a line, after an optional bullet/dash/bracket/list number, then the
title, or the title then a timestamp ending the line; `m:ss`/`h:mm:ss`; the list starts at the first
0:00 and runs while the times ascend; at least 3 chapters, each ≥ 10 s, else none:
`actions::watch::desc_chapters`) sit under the description in one panel that rests **compact**: a single
72 px row showing the chapter playing now (its storyboard frame, title, start time, "Chapter 3 of 30", a chevron),
updated in place when playback crosses a boundary (its text slides in from the side the new chapter lies on). It
**unfolds on hover** into the whole list *over* the content below (the section only ever reserves the compact row,
so the comments never move on any frame): one persistent overlay whose border layer, fill and content are clipped
from the compact row to the full list (clip-path on the house spring after a 60 ms intent delay, so a pointer
passing through never flashes it; the rows settle 6 px down into place); moving the pointer out folds it back
faster (180 ms ease-out). Only real pointer movement opens it (`data-track`), never the page scrolling under a
still pointer. Keyboard focus moving into it opens it with no motion, Escape folds it at once onto its compact row
(focus stays there; Tab opens it again), a click on the compact row folds/pins it (touch), and the player's chapter
label pins it open and scrolls it into view (a press elsewhere folds it). A row click seeks, and the chapter playing
now is marked (`li.on`, toggled in place when a boundary is crossed; its highlight springs between rows).
**Frames are the video's real content**: the storyboard (`player/storyboard.rs`) from the WEB `player` response's
`storyboards.playerStoryboardSpecRenderer.spec` (`base|w#h#count#cols#rows#interval_ms#name#sigh|…`, level `i` =
`$L`, `$N` = the name with `$M` → sheet number, `&sigh=`; the last sheet is cropped to its frames) gives every
chapter the frame taken first inside it, as a CSS background into the sheet (percentages, so the cell fills its
16:9 box exactly; other frame shapes letterboxed). The level is the smallest whose frames are at least the box's
device pixels (96 px boxes: 160 px frames at 1×, 320 px at 2×). The frames are written once per list after the
video's first frame (the sheets never compete with the start); without a storyboard YouTube's chapter thumbnails
are used. Sheets are cached by the browser only. Live storyboards are not used. **Comments** load when their section nears the viewport (observed only once the main thread is
idle, so they never compete with the player start): Top/Newest, replies on demand (collapsible, "Show
more replies"), paging, a compact **End** marker only after the last top-level page (never during
loading, sort changes, errors or empty/disabled notices), pinned/creator/heart/verified marks, links and
timestamps that seek our player, "(edited)" suffix dropped. Both comment formats are parsed (entity
`commentViewModel` + `frameworkUpdates`, and the older `commentRenderer`). Back/forward keeps a video's
loaded comments (memory, last 4 videos). **Top bar** (signed in): a bell with the unread count taken
from the `topbar` that page responses already carry (no polling), the list from
`notification/get_notification_menu` (More pages; opening clears the badge; a click marks it read via
`notification/record_interactions` and opens it in the client), and an avatar menu (name/handle from
`account/account_menu` on first open; Google Account, Switch account, Settings, Sign out: account pages open
on YouTube's own site).
All these popovers (Save, Share, bell, account) stay in the DOM: opening and closing flip a class, so each
grows out of its button: the panel's surface (`::before`, reaching up to the trigger) starts clipped to the
trigger's own box and radius (`bridge::morph_vars`, read once before the content is written) and opens to the
12 px panel (`--e-out`, `--d-panel`), border and shadow follow, and the rows settle in 25 ms apart (`.in`);
closing folds it back into the button, faster. The panel itself is never clipped or moved, so every row is where
it lands and takes clicks on the frame it opens; keyboard opens/closes (Enter, Escape) have no motion. Rows are
inset pills (their wash never reaches the rounded corners); long lists scroll inside the panel. Save and "Start
at" checkboxes fill and draw their check mark in (stroke-dashoffset on a persistent path; keys toggle at once);
Copy morphs into "✓ Copied" (one button, its label rolls up and blurs away, the check draws, the fill settles to
the tinted done look) and back after 1.8 s. The action-row status message unfolds from the row's right corner and
folds back into it when it expires.

### Speed

- **Preloading:** after the first screen, feed continuations are fetched in idle time with
  `priority: 'low'` until **100 videos** are in memory on Home and Subscriptions (one page ahead on
  search, channel and playlist pages). Scrolling moves them into the page 24 cards per DOM write
  (`content-visibility:auto` cards), so reaching 100 cards needs no network wait.
- **Instant reload / back:** feed pages (Home, Subscriptions, History, search, channel, playlist; last
  6) are kept as parsed data in **`sessionStorage`** (this tab only, gone when it closes; never
  `chrome.storage` or `localStorage`). A reload or a return from another page (e.g. YouTube's native
  watch page) paints the snapshot in the first document write, then the fresh response replaces it in
  place; if you have already scrolled down, a **New videos** pill offers it instead of moving the page.
  Back/forward within 30 min restores the snapshot at its scroll position without a request. Snapshot
  data is validated again on read like a fresh response and rendered through the same escaping.
- **First request** goes out at boot with this tab's config mirror (`sessionStorage`) or the built-in
  fallback; `chrome.storage` refines later requests (a different `au` session index redoes the page).
  The player still waits for `chrome.storage` (its preferences).
- **Thumbnails:** `srcset` width descriptors (`vi_webp/…/mqdefault.webp` 320w, YouTube's listed 360w and
  720w) with `sizes` per layout, so the browser takes the smallest sharp file (1x screens: ~9 KB vs
  14 KB per thumbnail, measured on i.ytimg.com; Retina keeps the 720w file); `fetchpriority=high` for the
  first row, `loading=lazy` below the first screen, `decoding=async`, `preconnect` to `i.ytimg.com` and
  `yt3.ggpht.com` in the shell.
- **Build:** `opt-level = "z"` (Wasm −23 %, no measurable speed change in the e2e timings).
- **Startup** (Chrome for Testing, fixtures, median of 30 loads, 1 Nov 2026): content script start → shell
  ≈ 1.4 ms: base64 decode 0.2, `WebAssembly.Module` 0.2 (V8 compiles lazily), instantiate 0.1, `boot()` →
  shell 0.8. Script start does not move with `content.js` size (a doubled payload started as fast), so size
  cuts do not buy startup; `wasm-opt` (binaryen 133: −8.6 % with `-Oz`) changed neither startup nor the Wasm
  CPU per flow beyond noise and is not used. Splitting rarely used parts would not help either.
- **Restored pages** (back to a feed) lay all cards out for the scroll jump (`.rs`, 1 s): cards are
  `content-visibility:auto` with a 240 px placeholder, so a freshly written page would otherwise land on other
  cards and then be moved by scroll anchoring.

**Player.** Watch pages play in our own `<video>` player (no YouTube player JavaScript, no iframe). The
streams come from the InnerTube `player` call with the `VISIONOS` client (direct googlevideo URLs, no
cipher, `n` or PO token, found by research on 1 Nov 2026), requested at navigation in parallel
with `next` and prefetched on hover. A second, normal `WEB` `player` call with your session supplies
captions and the watch-history URLs.

**Start path** (3 Nov 2026, measured live with `tools/bench/video.cjs`, numbers under "Speed" below):
- *Straight to the target rung* (≥ 1080p when the screen allows; 1080p is the floor): no low start rung.
  The old ≤ 360p start rung was replaced before its first frame showed anyway (live 2 Nov 2026: every
  start reported 360 → 1080 with HD at the first frame), so its 0.3–0.6 MB only took bandwidth from the
  1080p data.
- *Index + the start of segment 0 in one request*: the index range plus up to **128 KB** (video;
  audio 11 s of its bit rate, ≤ 128 KB). First segments are 33 KB … 4.4 MB at 1080p and their size is
  unknown until the index is parsed, so the guess is bounded; what of segment 0 came along is kept
  (`Track::prefix`) and only the rest is asked for. (512 KB cost ~150 ms on cold connections still in TCP
  slow start; a bit-rate-sized guess of up to 3 MB made the first frame wait for bytes past segment 0.)
- *decodingInfo before the first request*: answers are cached for the page (`CAPS`, memory) and asked
  as soon as a VISIONOS response is prefetched (`warm_caps`); a first video whose answers are missing
  waits for them at most 150 ms (seen: 2–55 ms), so no request goes to a codec that is about to be
  ruled out (before: up to 4 video heads in parallel, two of them wasted).
- *No layout read in the click task*: the quality cap comes from the screen; the player box (which
  can only raise it) is read on the player's tick, at most once a second. Before, `limit()` read the
  box five times right after the watch page was written, each a forced layout (≈ 13 ms, 52 ms at 4×
  CPU throttling; found by the boot work on 3 Nov 2026).
- *Hover media prefetch* (`own/pre.rs`): when the pointer has rested on a video link 150 ms past its
  VISIONOS answer, the index + up to 1 MB of segment 0 of exactly the formats the player will choose
  (same ladder, decodingInfo, size cap and ABR) are fetched; the click then appends at once (a body still
  loading becomes the player's own request). One video at a time, cancelled when the pointer leaves the
  link before it finished, kept in memory 5 min (last 3 videos). Live: ~1.2 MB per rested-on link.
  Focus counts like a resting pointer (the TV remote, keyboard focus): the bridge reports a link's
  `focusin` as intent in a microtask, since focus moved by Rust itself (`spatial_focus`) fires inside
  that call; a link whose VISIONOS response is already cached starts the dwell at once.
- *HDR only on an HDR screen*: HDR formats count as not smooth unless `(dynamic-range: high)`
  matches, so an SDR screen gets the SDR format of each rung (decodingInfo is also asked with an HDR
  format's transfer function and BT.2020). Live 3 Nov 2026 on the Android TV emulator decodingInfo
  called 1440p60 HDR AV1 smooth even so, and no picture came (7.6 s, MediaSource rebuilt); with the
  rule the same video starts in 1.2–1.5 s on 1080p60 SDR.
- *TV app*: googlevideo goes through the shell (Kotlin `HttpURLConnection` in `Host.kt`, the CORS
  header of rule 50), so `<link rel=preconnect>` would warm only the WebView's unused connection: on TV
  the warm-up is a `GET <host>/generate_204` (what YouTube's player warms with) through the shell. The
  shell keeps up to 16 keep-alive connections per host (Android's default is 5) and gives a host 6 s to
  connect / 15 s per read (was 15 / 30 s): `shouldInterceptRequest` holds a WebView request thread until
  the headers come, even after the page gave the request up (a hedge won elsewhere). Ranges are URL
  parameters, bodies stream through, aborts close the stream once headers are in.

**Slow YouTube** (2 Nov 2026: "if requests to YouTube would slow down like 100 times, we should have a
workaround, but still connecting with YouTube"; `player/throttle.rs` decides, `own/hedge.rs` acts; nothing
here leaves the player):
- *Player request hedge*: no VISIONOS answer after **400 ms** → the same request again; the first usable
  answer plays, the other is aborted (an error from one waits for the other). 400 ms ≈ 2 × the owner's
  p90 (205 ms) and above p99 (348 ms) of 158 answers: it fires on ~1 % of starts and turns a hung request
  into ~0.5 s instead of the 8 s timeout. Racing another client is not done: no other client gives direct
  URLs that play past ~60 s without a PO token (see "Bot check recovery").
- *googlevideo requests stream* (the bridge's generic `progress` fetch option: headers, then bytes so far
  every 100 ms), so a slow host shows early. A request is *throttled* when no headers came after **700 ms**
  before the first frame (the slowest cold connection seen: 639 ms) or 1.5 s while playing with < 12 s
  buffered, or when after ≥ 0.5 s of body it runs **6× slower than the measured bandwidth**, or (before
  the first frame / with < 12 s buffered) at **under half the media's bit rate**, with > 0.8 s left.
- *Recovery, in the owner's order*: (1) the same range on the next mirror the URL lists (`mn`/`fvip`,
  `fallback_count`, as YouTube's player does); both run, the first full answer wins, and later requests
  go to the winner's host; (2) when the mirror twin is slow too (or there is no mirror): fresh URLs (a new
  player response); (3) another client: none qualifies (above); (4) only with a measured rate: the highest
  rung that rate sustains (70 %), held 15 s (then 30, 60, 120 s if it comes back), after which Auto goes
  back to ≥ 1080p by itself. A host that sends nothing at all is left to the stuck schedule ("Stalls are
  not failures"). A cached player response whose URLs hang still gets fresh URLs at 0.8 s instead of a
  mirror (those URLs are the usual culprit).

Controls (24 px outline icons): play/pause, mute + volume, time, then the shortcuts **CC** (thin outline
when off, filled cut-out letters when on: the old extension's artwork), **Speed** (shows the rate as text:
`1×`, `1.25×`), **Quality** (shows the rung playing now in a small frame: `360`, `1440`, `4K`), theater,
fullscreen. The text buttons have a fixed width, so a label change never moves the toolbar. **No loading
indicator** (1 Nov 2026): the video's thumbnail (the card's `vi_webp/…/mqdefault.webp`, usually
already cached) is the poster at the final geometry until the first frame replaces it. The three shortcuts **open on hover** (16 ms intent; leaving closes
after 400 ms, or 180 ms once the pointer was in the menu; travel with a button held is not intent and a
held slider is never dismissed). In CC hover never selects anything; **Speed and Quality choose with zero clicks**
(since 2 Nov 2026, see below). A click pins a menu (a second click, Esc
or a click on the video closes it); after a mouse choice it closes when the pointer leaves. Enter/Space on
a shortcut opens it with focus on the current choice, arrows move, Esc returns focus to the shortcut.
**Motion** (motion law; all class toggles on the persistent `#ytc-player`, transform/opacity only): the
menu is one persistent panel: the first open fades + scales from `.97` at its shortcut (170 ms ease-out,
clickable on its first frame), moving between shortcuts only swaps the content (no re-animation), it
closes in 110 ms; choices toggle classes in place (the CC switch thumb slides on the house spring). The
control bar fades out on idle or when the pointer leaves the player while playing and comes back on the
same frame; a stall over 0.6 s brings it back softly and the empty track breathes mint (no spinner).
The seek track grows with `scaleY` (instant in, eased out), the knob rides on `translateX`; while playing
the fill glides at real speed (Rust aims `--p` one 250 ms tick ahead), seeks/drags/pauses land 1:1.
**Chapters on the player** (like YouTube): the seek bar splits into one segment per chapter with
2 px gaps (placed once per video on whole pixels; every fill is the same `--p`/`--b`/`--h` mapped into
its segment, so per tick nothing more is written); hovering shows a tooltip above the bar (the storyboard frame
at the pointer's time, 160×90, written only when the frame changes; then chapter title over the time; the time
alone without chapters, no frame without a storyboard) centered on the pointer and clamped inside the bar, a
lighter hover fill, and the segment under the pointer grows taller (`.on`, instant). The pointer is
reported by the bridge's generic `data-track` (rect read once on enter). The current chapter sits beside
the time (`0:41 / 12:03 • Title ›`, text written only on a boundary); clicking it opens the chapter list
and scrolls it into view on the current chapter (YouTube opens its chapters panel; in fullscreen it
leaves fullscreen first). Ctrl/⌘+← / → jump to the previous (or the current chapter's start when > 3 s
into it) / next chapter, with the title as the toast; without chapters these keys stay the browser's.
Buttons scale on press; toasts appear on the key's frame and fade out; a new caption cue fades in
quickly (lines that stay never move). Keyboard shortcuts add no motion.

- **Speed**: a vertical list centred above its shortcut, **1×** at the bottom (next to the shortcut), then
  1.25, 1.5, 1.75, 2, **2.5×** at the top, under a header − exact rate + (0.05 steps, 0.25–2.5×; slower
  rates live there). `<`/`>` step 0.25 and stop at 2.5×. The shortcut always shows the rate.
- **Quality**: a vertical list centred above its shortcut, **Auto** at the bottom, then the rungs worst →
  best going up (Premium/enhanced at its numeric tier); a dot marks the rung playing now. A compact codec
  row (Any / VP9 / AV1 / H.264, only the supported ones) on top when there is a choice.
- **Zero clicks** (2 Nov 2026: fewer clicks; the wheel or a resting pointer chooses without clicking;
  this overrides the hover-never-selects rule for these two menus).
  *Wheel:* over the Speed or Quality shortcut (open or not) or anywhere in its menu, the wheel chooses and
  never scrolls the page (`bridge::wheel_take`, preventDefault on `[data-wheel]` only): up = the next value
  up the list (faster / better), down = down; over the Speed readout it steps 0.05. Mouse-wheel clicks
  are one step each; trackpad pixels accumulate (`ui::Wheel`: first step after 3 px, the second after 50,
  each later one 40 px dearer, a pause of 120 ms starts a new gesture), so a swipe moves a few rows and a
  flick with momentum a handful, never the whole list. Speed applies per step; Quality moves the
  selection mark and the toast every step but switches the stream once, 200 ms after the wheel rests (the
  rows passed are never fetched). At either end the list (or the closed shortcut's label) nudges 4 px
  once per gesture and the toast repeats the value. A short player scrolls the list itself, following the
  selection. *Dwell:* resting the pointer on a row for 220 ms (`ui::DWELL_MS`) chooses it; the row fills
  left to right over that time (a linear clip-path wipe; reduced motion fades it), moving to another row
  restarts it, and rows passed over on the way (the bottom ones, entering from the toolbar) never select.
  A row that came under a still pointer by itself (the list scrolled after a wheel step, the panel opened
  from the keyboard) starts no dwell for 250 ms. A click still chooses at once; the keyboard is unchanged.
- **CC**: caption languages, then a full-width remembered **Captions On/Off** switch. An **Audio** tab
  (switches on hover) appears only when the video has several audio tracks; choosing one cuts the new
  language in about 1 s later through the normal quality-switch path.
- **SponsorBlock** (2 Nov 2026; `player/sponsor.rs` pure, `player/own/sb.rs` glue): sponsor
  segments from SponsorBlock's public database are skipped in our own player (the SponsorBlock extension cannot act
  on it). *Lookup:* k-anonymous: `GET https://sponsor.ajay.app/api/skipSegments/<first 4 hex of SHA-256(video id)>`
  with every category (`sponsor`, `selfpromo`, `interaction`, `intro`, `outro`, `preview`, `music_offtopic`,
  `filler`, `poi_highlight`) and action types `skip`/`poi`; the answer lists every video in that bucket (~150) and
  ours is picked locally. SHA-256 is a small pure-Rust implementation (NIST vectors in the tests). Sent with the
  player's own requests at start, after the two `player` calls, at low priority; nothing waits for it (first frame,
  seeking and chapters are independent); no answer in 6 s, an error or garbage → nothing happens. *Skipping:*
  checked on the player tick, on `timeupdate` and by one timer aimed at the exact start when the next skip is less
  than a tick away. Playing into a chosen segment (from 0.1 s before its start) seeks to its end, carried through
  chosen segments that touch or overlap it; a toast slides in from the left: **Skipped sponsor · Undo** (5 s; Undo
  seeks back to the start and that span no longer auto-skips in this video). A seek by the user *into* a segment
  only shows it (until playback leaves it); the start position (resume, `t=`) is not such a seek. Defaults like
  SponsorBlock's: sponsor, self-promotion and interaction reminders skip; intro, outro, preview, non-music and
  filler only show. *Seek bar:* every range is drawn in its category's color inside the chapter segments (above the
  fills; they fade in when the data arrives), the highlight as a pink tick, and hovering a range names it in the
  tooltip; a highlight further ahead is offered once as **Highlight at m:ss · Jump**. *Menu:* a shortcut left of
  CC (an arc jumping over the track; slashed when off) opens like the others: **Auto-skip** choices in two columns
  with this video's counts, then a remembered **SponsorBlock** On/Off switch (off: no lookup at all, no ranges, no
  skips). Remembered in `ytcConfigV1` `p`: `sbo:false` only when off, `sb` (category keys) only when not the
  default.
- **Caption language**: the source language is YouTube's speech-recognition (ASR) track's
  language. Russian or English source → captions in that language; any other source → an offered English
  track, else YouTube's English translation of a source-language track (`&tlang=en`, listed as
  "English (translated)"); human tracks before ASR. No ASR evidence (or conflicting) → YouTube's default
  track (`defaultCaptionTrackIndex`), else the old rule. A language picked in the CC menu is kept for that
  video for this tab session (memory only, 100 videos); other videos keep the rule. On/Off stays remembered.
- **Captions**: cyan Arial with a dark outline, no background, 75 % of the base size. Drag the text to
  move it; a slim vertical range at its right edge (shown on hover/focus) scales it 0.5–2×; the **mouse
  wheel** over the caption (or the range) resizes it too, 5 % a notch, up = larger (trackpad deltas add up
  to a notch; the page does not scroll; Ctrl/⌘+wheel stays zoom), and ↑/↓ (←/→) step it 5 % while the range
  is focused (the old extension's behaviour). Both are one object in storage (`p.cs = {v:2, x, y, s}`:
  upper-left corner as player fractions, scale 50–200 %), written only after a deliberate drag/resize
  (batched). Default position: near the upper-left.

Keys (YouTube's): space/`k`, `j`/`l` (±10 s), ←/→ (±5 s), ↑/↓ (volume ±5 %), `f`, `m`, `c`, `t`,
`<`/`>` (speed); `0`–`9` are **blocked** (2 Nov 2026, as in the old extension: taken on watch pages
and do nothing, no jump to 0–90 %, not in the `?` sheet); Ctrl/⌘+←/→ (previous/next chapter; the bridge reports
Ctrl/⌘ keys outside text fields as `Mod+<key>`, unhandled ones stay the browser's). Captions on/off (default **on**), volume/mute, quality,
codec, theater and caption placement/size are remembered (`p` in `ytcConfigV1`; writes are batched
300 ms so slider gestures write once).

**Live streams** play in our player too (`player/live.rs`). VISIONOS returns an HLS master playlist whose
variants are MPEG-TS video with separate packed-ADTS audio, and `adaptiveFormats` without byte ranges;
those adaptive URLs serve the same segment numbers as self-initializing fragmented MP4 with `&sq=N`
(checked 1 Nov 2026 on two public streams; audio and video share one timeline). So the timeline comes
from HLS (master → the variant nearest the playing rung → media playlist: sequence numbers, durations,
sliding window, `#EXT-X-ENDLIST`; refreshed every target duration, half of it when nothing changed;
~14 KB gzip for a one-hour window) and the segments are fetched as fMP4 and appended to the same MSE
pipeline as VOD (`ftyp`+`moov` when the rung changes, then `moof`+`mdat`; `emsg` dropped). No TS→fMP4
remux is needed. It starts two target durations behind the newest segment (10 s on 5 s segments: about
16 s behind the broadcast, measured from YouTube's `Ingestion-Walltime-Us`), follows the edge (jumps back
when > target + 15 s behind after a stall or hidden tab), and the seek bar/keys/`j`/`←` seek within the
playlist window (DVR); the time shows `−m:ss` behind live and **LIVE** (red dot at the edge) jumps back.
ABR, the size cap and remembered quality apply as for VOD. A stream whose playlist ends plays to its last
segment and ends normally. Upcoming streams and premieres (`LIVE_STREAM_OFFLINE`/`isUpcoming`) show the
failure panel with YouTube's own words ("Premieres in 2 hours") without an incident; a live stream without
fMP4 formats or playlists shows the panel with the reason as an incident. Live chat and live captions stay on YouTube's page (not built).

**Next video** (for the playlist / Up next logic): when playback ends the player emits `Out::Ended`
(the app marks `ytcEnded` and calls `App::on_ended`, an empty hook to fill); `Own::hint_next(id)` makes
the app prefetch that video's `next` and VISIONOS responses once, so starting it needs only googlevideo.
Reusing the `<video>`/MediaSource was measured and not built: a new element + MediaSource + two
SourceBuffers is ready in 0.2–0.5 ms (4 ms only the first time in a tab).

**Failures stay in the player; the client never leaves for YouTube's page by itself** (2 Nov 2026: an
automatic switch to YouTube's page after a few seconds hid problems; always staying in the client's own player
means every failure can be seen and inspected where it happens; `src/player/fail.rs`). If our player cannot play a video (VISIONOS not OK: age-restricted,
members-only, private, unplayable, the bot check on every route; upcoming streams/premieres; no compatible
format; googlevideo 403 again after a `player` refresh; index/parse errors after their retries; an
MSE/media error; a stuck episode that never recovered), it releases the media and the app shows a **panel
in the player box** over the dimmed poster (it settles in: `@starting-style`, no spinner): a one-line
message, the technical reason in smaller, selectable monospace text, and two buttons. **Try again**
re-mounts the player for the same video and time with a fresh VISIONOS call (never the cached or a
prefetched response; it also lifts the bot-check block, so every route is tried again). **Open on
YouTube** opens YouTube's own watch page at that time (`/watch?v=ID&t=Ns&yt-native=1`); it is the only
way a watch page goes native. Enter/Space on a focused button work (native button activation). The URL
and the page stay; the reason is kept in `chrome.storage.local` (`ytcPlayerLogV1`, last 10) and shown in
the debug overlay (`player failed …`). Real failures are `player-failed` incidents with the full player
report (was `player-fallback`); expected ones (upcoming, age-restricted, members-only, own player off)
show the panel without an incident. YouTube's embed is no option: on a youtube.com page it shows "Error
code: 152 - 4". Messages (`fail::message`, unit-tested):

| Reason | Panel message |
|---|---|
| upcoming / premiere | YouTube's words, e.g. "Premieres in 2 hours" (else "This live stream hasn't started yet") |
| `VISIONOS LOGIN_REQUIRED` bot check | "YouTube asks to sign in to confirm you're not a bot" |
| `VISIONOS LOGIN_REQUIRED` age | "Age-restricted — sign-in needed" (other sign-in reasons: "Sign-in needed to watch this video") |
| any reason mentioning members | "Members-only video" |
| `VISIONOS ERROR` | YouTube's words (e.g. "This video has been removed by the uploader"), else "Video unavailable" |
| `VISIONOS UNPLAYABLE` | "YouTube won't play this video here" |
| `VISIONOS CONTENT_CHECK_REQUIRED` | "YouTube asks to confirm before showing this video" |
| VISIONOS HTTP error / no answer in 8 s | "YouTube's player service didn't answer" |
| no compatible formats | "No format this browser can play" |
| media error, appendBuffer, SourceBuffer, changeType | "The browser couldn't play this video" |
| live playlist / segment errors | "This live stream isn't loading" |
| own player turned off (`op: false`) | "The client's own player is turned off" |
| anything else (googlevideo, index, stuck to the end) | "This video isn't loading" |

**Stalls are not failures** (`fail::START`, `fail::PLAY`, `own.rs` `stuck_tick`). No first frame
**4 s** after the streams were chosen, or buffering **15 s** mid-playback (after the 3 s rung step-down),
begins a *stuck episode*: a small non-blocking status at the player's top right, "Still loading —
retrying…" with smaller Try again / Open on YouTube buttons (fades and settles in, fades out faster; only
its buttons take the pointer), and one `player-stuck` incident with the full report. The player keeps
retrying in place with **fresh player URLs** (a new VISIONOS response; requests in flight are dropped and
asked again with the new URLs; a non-OK answer to such a retry is not a verdict). Start: 0.7.6's refresh
at 2.5 s when googlevideo never answered, then retries at **6 s** and **15 s** (if googlevideo answered but
no picture came, the 6 s retry also rebuilds the MediaSource and SourceBuffers once), the panel at
**30 s**. Playback: retries at **15 s** and **25 s** of buffering (paused: the schedule waits), the panel at
**40 s**. When the picture comes or playback resumes, the status fades and nothing else changes; a
`player-recovered` incident records the outcome (`… recovered after 3.2 s (1 retry with fresh URLs)`);
when it never does, the `player-failed` incident is the outcome. A background tab's start waits until the
tab is shown, as before.

**Bot check recovery** (`own.rs` `recover`, `api.rs` `VisionRoute`). When VISIONOS answers
"Sign in to confirm you're not a bot", the player sends the other routes **in parallel** at once:
VISIONOS with **your account session** (cookies + `SAPISIDHASH`, only when signed in; one extra round
trip) and a **fresh anonymous visitor** (`/youtubei/v1/visitor_id` without cookies, ~40–60 ms, then
anonymous VISIONOS with it). The first OK plays; the other answer is ignored. The route that recovered
is kept **for the rest of the tab session** (memory only; the fresh visitor is never stored) for later
videos and for hover/speculative prefetches. A prefetch that meets the bot check is not a verdict: it
holds further VISIONOS prefetches until the player's own request (which recovers) succeeds. Only when
every route was bot-checked does the player show the panel ("YouTube asks to sign in to confirm you're
not a bot"), with the steps in the reason (`… · tried: anonymous (bot check), account session (bot
check), fresh visitor (bot check)`); for 10 minutes then, no VISIONOS prefetches, and a new video tries
the remembered route only (no parallel recovery; blocked again → the panel, whose Try again recovers
fully; an OK answer lifts the block). The happy path is unchanged (one anonymous VISIONOS call).
Other clients are no fallback: on 2 Nov 2026, from a network that was bot-checked, every anonymous client
(VISIONOS, ANDROID_VR, IOS, ANDROID, TV/Tizen, WEB, MWEB) got the same bot check with fresh and old
visitors, and IOS/ANDROID/ANDROID_VR also stop after ~60 s of media without a PO token.

**Debug overlay:** press `` ` `` or `Shift+D` (outside text fields). It shows the client version,
visitor/sign-in state, parse counts (Shorts parsed, ads skipped, duplicates, and every **unknown
renderer** YouTube sent that we don't handle yet), cache sizes, timing marks, and the player's state (formats, first frame, buffer ahead, throughput,
pings, its event notes, a stuck episode) plus recent player failures.

## Hook points (for feature modules)

Other modules plug into the markup without editing `render.rs` or the player:

| Hook | Where | How |
| --- | --- | --- |
| `render::set_card_hook(fn(&mut String, &Video))` | every video card (feeds, search, related), inside `.d` | Call once at boot. Markup with class `ca` sits centered beside the title (e.g. the Watch later button); anything else follows the metadata lines. Cards carry `data-v="<videoId>"` on `.c`. |
| `#ytc-filters` (`render::FILTERS`) | right side of the heading row on every feed page (Home, Search, Subscriptions, …) | Fill with `bridge::html` after the page renders. On Home/Search the visible title is hidden, so the row is only as tall as its content. |
| `#ytc-sub` (`render::SUBSCRIBE`) | watch page, beside the channel name | Inside `#ytc-info`, which is rewritten when `next` arrives: fill after `render_watch`. Hidden while empty. |
| `#ytc-acts` (`render::ACTIONS`) | watch page, right end of the channel row (Like/Dislike/Save…) | Same lifetime as `#ytc-sub`. Wraps on narrow widths; hidden while empty. |
| `#ytc-comments` (`render::COMMENTS`) | watch page, below the description | Outside `#ytc-info`, so it survives the info re-render. Hidden while empty. |
| `data-act` / `data-arg` | any element | Clicks go to `App::on_click(href, act, arg, mods)` (bit 32 of `mods` = keyboard activation). `p-*` acts belong to the player. |
| `data-hov` / `data-arg` | any element | The bridge reports the hovered zone to `on_hov(zone, arg, held)`; `MenuCtl` in `player/ui.rs` is a ready-made intent/grace state machine for hover-open popovers (the Filters and Save menus reuse it with their own timings). |
| `bridge::focus_in(id, delta)`, `bridge::focus_grid(id, sel, delta)` | keyboard | Focus the selected/first/next control in a popover; roving focus over a list (±1, ±1000 = one row). |
| `bridge::morph_vars(btn, pop)` | motion | A popover grows out of its trigger: writes the trigger's box relative to the panel (`--bw --bh --br --up --bx`) on the panel; client.css clips the panel's surface from it. Layout offsets, read once at a pointer open (before the content is written) or close. |
| `bridge::click_rect(scope, inner)`, `bridge::rect(id)`, `bridge::flip_from(id, x, y, w, h, easing, ms)`, `bridge::fade_in(id, easing, ms)` | motion | Shared-element morphs (FLIP): read the source rect before a swap, write the page, then paint the destination from that rect (compositor-only). `fade_in` only on content just written. Easings/durations come from `motion.rs`. |
| `bridge::stagger(id, sel, sub, max, step, dy, easing, ms)`, `bridge::nudge(id, dy, easing, ms)`, `bridge::pill_slide(scope, from, to, cls, easing, ms)`, `bridge::pill_carry(scope)`, `bridge::flip_mark(scope, sel)`, `bridge::flip_play(scope, sel, easing, ms, out)` | motion | Entrance wave over freshly written items (no layout read); a persistent element's new text sliding in; a selection pill sliding between two items of a row (an empty `to` stops a running slide; `pill_carry` after re-rendering the row); reflow FLIP around a re-render that removes/moves items (keyed by `data-k`, else text). Reads happen only at the click. |
| `bridge::wheel(id, name)` → `on_wheel(name, dir)`, `bridge::read_value(id)` | input | Wheel notches over one element (−1 up / 1 down; trackpad deltas add up to a 50 px notch; a handled one does not scroll the page; Ctrl/⌘ stays zoom), bound once per element. The `value` of a textarea/input (the import paste box). |
| `#ytc[data-still="1"]` | CSS | Set during the first frame of keyboard and back/forward paints: gate every `@starting-style` entrance with `#ytc:not([data-still="1"])`. |

Shared design tokens are CSS variables on `:root` (client.css, one palette, no stray hex colors):
surfaces `--bg --pn --rs --cb` (background, panel/popover, raised, control), `--hv` (the one hover wash),
`--on`/`--onh` (selected row tint / its hover), `--fi --bd` (text field, field/outline border), `--tk`
(switch track), `--ln` (hairline), `--wa` (translucent wash over video), `--sc` (scrim behind badges and
toasts); text `--tx --mu --fa` (≥ 10:1, ≥ 5:1, ≥ 4.5:1 on bg/pn/rs), accent `--ac` `#b7ebca` / `--ach`
(hover), `--rd` (live, destructive), `--er` (error text); `--sh` (popover shadow); radii `--r-card` 14,
`--r-pop` 12 (popovers, panels), `--r-ctl` 10 (chips, buttons); press depths `--pd` .97, `--pd-card` .985,
`--pd-icon` .95 (icon-only buttons), `--pd-glyph` .84 (an icon squeezing inside its control). Selected:
rows get `--on` + mint text; chips and segments solid `--ac` with `--bg` text.

## Moving from the old YourTube extension

An earlier, private YourTube extension (a native-YouTube restyling) kept its data in its own storage. The client
can import it once: **+ Topic** → **Import data from the old YourTube extension…**, run the line the dialog shows
in the old extension's service-worker console (`chrome.storage.local.get(null, d => copy(JSON.stringify(d)))`,
nothing is sent anywhere), paste, **Import**.

What happens (`src/features/import.rs`, pure Rust, unit-tested): the paste must be one JSON object of
at most 4 MB with at least one YourTube key, and every known key must have the old extension's exact shape,
else nothing at all is written and the dialog says which key was wrong. Single bad records inside a good
store (an invalid domain, a broken resume entry) are skipped and counted. The client's bounds apply: 100
searches, 2,000 resume records, 50 blocked websites, 20 topics, 20 YouTube-topic records.
- Merged: search history and resume positions (newest wins per query / per video, the newest 100 / 2,000
  kept); blocked websites, your topics and YouTube's chip order/names/hidden state (union, the client's own
  entries first and winning on conflicts).
- Taken from the old extension: the Home filter settings (`yourTubeMinimumHomeDurationV1`,
  `yourTubeHomeOrderV1`, `yourTubeHomePlaylistsOnlyV1`, `yourTubeShowMusicV1`, `yourTubeShowLiveV1`,
  `shortsInFeeds`) and the captions: `yourTubeCaptionsV1` `{version 1|2, position {x, y}, scale}` becomes the
  player's `ytcConfigV1.p.cs` `{v:2, x, y, s}`, `yourTubeCaptionsEnabledV1` becomes `ytcConfigV1.p.cc`.
- Left out: `yourTubeStatsV1` (viewing statistics) and the legacy `yourTubeDiscoveryTopicsV1` /
  `yourTubeHomeTopicsV1`; any other key is ignored.

The old data holds no descriptions, recommendation payloads, continuation tokens or verdicts, and none are
created. Importing twice is harmless (the second run merges into the same records).

## Code map

| Piece | What it does |
| --- | --- |
| `manifest.json` | MV3; `storage` + `declarativeNetRequest`; host `https://www.youtube.com/*` (plus `*.googlevideo.com` for the player and `https://sponsor.ajay.app/*` for SponsorBlock; `https://api.deepgram.com/*` for voice search's key check); two `document_start` content scripts: the client (with exclusions) and native mode (`*yt-native=1*`: `native.css` + `content.js`). |
| `native.css` | YouTube's own pages only: the old extension's hiding/theming rules (see "YouTube's own pages keep the client's setup"). |
| `src/native/` | Native pages: `mod.rs` returns to the client when leaving the fallback video; `clean.rs` the cleanup logic (captions, keys, CC, promotions, Shorts switch). |
| `content.js` (generated) | `window.stop()` first, then the bridge, the wasm-bindgen glue and the Wasm bytes (base64 in a block comment, read via `Function.prototype.toString`), `initSync`, `boot()`. |
| `bridge.js` | Generic primitives only: replace the document, set/append HTML/text by id, attributes, CSS variables, history, `location.assign`, scroll, fetch (bytes back to Rust, optional init options; `progress: 1` also reports headers and bytes so far), abort, `chrome.storage`, timers, IntersectionObserver, postMessage, fullscreen, element size, `fade_imgs` (images still loading fade in when they land; cached ones are never hidden); media objects by numeric handle (bind a `<video>` and forward its events, `MediaSource`, `addSourceBuffer`, `appendBuffer`, `remove`, `changeType`, `buffered`, get/set/call properties, `isTypeSupported`); delegated click/dblclick/submit/pointerover/pointermove/keydown/input/drag/popstate/pagehide; voice: microphone → 16 kHz PCM chunks with a level custom property, a WebSocket relayed by the background worker (a stored secret appended there), a held bare key, contextmenu on `[data-ctx]`, secrets (only whether one is set; saved straight from an input). No YouTube or player knowledge: every decision is in Rust. |
| `rules.json` | Static DNR. YouTube's site bundles are not blocked (YouTube's service worker fetches them outside the page frame, so a block broke the native fallback; `window.stop()` keeps them from running on client pages). `allowAllRequests` for `yt-native=1` and the native paths at priority 2; block `log_event`, `/pagead/`, `csi_204` and doubleclick (from youtube.com) at priority 3, so telemetry stays blocked even on native pages; rule 50 sets `Access-Control-Allow-Origin: *` on googlevideo responses to youtube.com, because googlevideo randomly redirects chunk requests to another node and the redirected request carries `Origin: null`. `/s/player/` and `/api/stats/` are never blocked. `background.js` handles auto-update, debug-center delivery and the voice socket relay. |
| `client.css` | The whole look (charcoal/mint tokens, layout, player), injected by the manifest (no flash). 15 KB (4.7 KB gzip). |
| `src/route.rs` | URL → page. |
| `src/config.rs` | Replaces the stopped page's `ytcfg`: client version + visitor data from `/sw.js_data` (2.7 KB, the endpoint YouTube's own service worker uses), cached in `chrome.storage.local` key `ytcConfigV1` for 24 h; fallback version `2.20261030.01.00`; session index `au` (default 0); SAPISID cookies → `SAPISIDHASH`/`SAPISID1PHASH`/`SAPISID3PHASH`. |
| `src/api.rs` | InnerTube bodies and headers (`X-Youtube-Client-Name/Version`, `X-Origin`, `X-Goog-AuthUser`, `X-Goog-Visitor-Id`, `Authorization`). Same-origin `fetch`, cookies included by the browser. |
| `src/parse.rs` | **All** response parsing: `videoRenderer`, `compactVideoRenderer`, `gridVideoRenderer`, `playlistVideoRenderer`, `lockupViewModel` (video/playlist/podcast/album/course), `channelRenderer`, legacy playlist renderers, `continuationItemRenderer`/`continuationItemViewModel`, watch primary/secondary info, attributed descriptions (UTF-16 offsets, redirect links unwrapped, only http(s) kept), channel/playlist headers. A walker descends only through known containers; Shorts (`shortsLockupViewModel`, `reelItemRenderer`, Short `videoRenderer`/`lockupViewModel`, Shorts shelves) become video items marked `short` and are counted; ads are dropped and counted; unknown renderers are counted, never fatal. |
| `src/json.rs` | The JSON reader for every response and stored record: accepts and reads exactly what `serde_json` would (same numbers, members in key order, last duplicate wins, same errors; checked against serde_json on the fixtures and 3,000 mangled documents), but borrows unescaped strings from the body and keeps objects as sorted vectors. Halves the Wasm CPU per flow (e2e fixtures: search load 10 → 4.5 ms, Home preload to 100 videos 34 → 18 ms, hover + open a watch page 37 → 20 ms). `serde_json` only builds outgoing JSON. |
| `src/description.rs` | Pure port of the old extension's description rules (`resource`, line walk, bare-link labels), unit-tested case for case against the old extension's cases. |
| `src/render.rs` | HTML strings; every text/attribute escaped; one `innerHTML` per region; event delegation on the document. |
| `src/player.rs` | The player boundary (the app only talks to this): markup for the player box, the manual Open on YouTube URL, and `Own`; `player/fail.rs` the failure panel, its messages and the stuck episodes' retry schedule. |
| `src/player/own.rs` | Our player's state machine: VISIONOS + WEB `player` calls, format choice, index fetch (with the start of the first segment in the same request), hover media prefetch (`own/pre.rs`), hedging and throttle recovery (`own/hedge.rs`), per-track fetch/append queues, seek, quality/codec switch (`changeType`), eviction, end of stream, 403 → one refresh, retries, stall/first-frame watchdogs, captions, pings, controls and keys. |
| `src/player/{formats,index,sched,stats,captions,storyboard,throttle,ui}.rs` | Pure, unit-tested parts: when YouTube is slow (hedge deadlines, the throttle verdict, escalation order, the drop rung and its hold); storyboard spec parsing, level choice and frame/cell math (real WEB spec fixture `player_web_storyboard.json`); format ladder (codec order by `isTypeSupported`, size cap = player height × devicePixelRatio with 20 % slack, throughput ABR at 70 % with a 1080p floor; the ≤ 360p start rung is used by live streams only); MP4 `sidx` and WebM `Cues` parsing; buffer decisions (30 s ahead, evict 60 s behind, seek resume, gap-free switch cut); history ping URLs as base.js `Q4a` builds them (`s.youtube.com` → `www.youtube.com` like the web player's `vss_host`); json3 captions; control markup, the three menus and the hover-menu state machine (`MenuCtl`). |
| `src/app.rs` | State: navigation (pushState/popstate), request lifecycle (stale responses dropped, aborts), feed queue (shown / preloaded / continuation, generation-checked), idle preloading, snapshots (stale-while-revalidate), hover prefetch, previews, suggestions, in-memory caches. |
| `src/actions/` | Watch-page actions, comments, chapters, notifications, account menu: `watch.rs` (like/subscribe/comment-token/chapter data from `next`, row markup), `comments.rs` (both comment formats, markup), `save.rs` (playlist membership from `get_add_to_playlist`, chooser), `notify.rs` (topbar count/avatar, notification list, account menu), `req.rs` (request bodies, outcome/reason), `toggle.rs` (optimistic state: one request in flight, queue a change of mind, roll back on failure), `mod.rs` (state, popovers, timers, responses). Unknown comment items are counted (debug overlay line `acts`). |
| `src/telemetry.rs`, `src/app/telglue.rs`, `src/player/own/tel.rs` | Speed telemetry (see "Speed telemetry" below): the batched event buffer, its gate and frame spans (`telemetry.rs`, unit-tested); the app's hooks (navigations, InnerTube requests, flushes) and the player's (start breakdown, googlevideo requests, seeks, switches, stalls, drops, menus, toasts, session summary). The bridge only observes (`tel_*`), buffers and forwards. |
| `src/snap.rs` | Feed snapshots ↔ JSON for `sessionStorage`: compact item records, re-validated on read (ids, hrefs, image hosts), size caps (150 items, 400 KB, 6 pages). |
| `src/features/*.rs`, `src/app/fxglue.rs` | The extension's features rebuilt natively (see "Features" below): pure, unit-tested logic per feature (`filters`, `history`, `progress`, `websites`, `watch_later`, `topics`, `search_filters`, `home_chips`, `upnext`); `fxglue.rs` (a child module of `app`) wires them in with one-line hooks in `app.rs`; `app/upglue.rs` owns what's next on watch pages, `app/speed.rs` the speculative/background prefetch and the perf report (`src/perf.rs`). Cards get the Watch later button through `render::set_card_hook`; the filter bar fills `#ytc-filters`. |

Privacy (features): the keys below are the only additional records in `chrome.storage.local`:
`yourTubeMinimumHomeDurationV1`, `yourTubeHomeOrderV1`, `yourTubeShowMusicV1`, `yourTubeShowLiveV1`,
`yourTubeHomePlaylistsOnlyV1`, `shortsInFeeds` (the Shorts switch, boolean; the old extension's key for it), `yourTubeHistoryV1` (last 100 searches), `yourTubeProgressV1` (≤ 2,000 resume
records: id, position, duration, completed, time), `yourTubeBlockedWebsitesV1` (≤ 50 domains),
`yourTubeRecommendationTopicsV1` (≤ 20 topics), `yourTubeNativeTopicsV1` (order/hide of YouTube's own Home
chips: `{version:1, topics:[{key, name, hide?}]}`, ≤ 20 records). The Autoplay switch is `ap` in `ytcConfigV1` (written only when
off). Never stored: descriptions, topic cards, chip/continuation tokens, search-filter params, website verdicts (description *hosts* for ≤ 500 videos are kept in memory only). The filter
preferences are mirrored in this tab's `sessionStorage` (`ytc:fx`) so a reload filters before storage answers.

Privacy (voice search): the outside service **Deepgram** receives microphone audio only while a dictation runs (from
the dwell / key press to the end; never otherwise) plus the user's key in the WebSocket handshake. The key is stored
only in `chrome.storage.local` `ytcVoiceKeyV1` (local, not synced), read only by the background worker, never in the
page, Wasm, logs, incidents, the debug overlay or the repo (e2e checks storage, incidents, the page and the overlay).
Transcripts live in the field until the search; nothing of the audio or text is kept.

Privacy: only `ytcConfigV1` (client version, visitor data, session index, refresh time, player preferences) and
`ytcPlayerLogV1` (the last 10 player failures: time, video id, position, reason) are persisted. A fresh visitor from the bot-check
recovery and the chosen VISIONOS route live in memory only (never stored, never in reports).
SponsorBlock (an outside service, besides YouTube itself): requests carry only the 4-hex SHA-256 prefix
of the video id (never the id, no cookies `credentials:"omit"`, no referrer `referrerPolicy:"no-referrer"`, no HTTP
cache `cache:"no-store"`); answers are kept in memory for this tab (100 videos) and never persisted; skips are not
reported back (no `viewedVideoSponsorTime`, no votes, no user id). Turning it off sends nothing at all. Only the
two small preferences above are stored.
Feeds, descriptions, recommendations and continuation tokens live in memory, plus feed snapshots (items,
continuation token, scroll) and a config mirror in this tab's `sessionStorage` (cleared when the tab
closes; YouTube's own pages in the same tab share that origin storage).

## Features (ported from the extension)

- **Watch later button** on every video card (feeds, search, related; signed in only): a clock in the card's
  `ca` slot. First hover/focus asks `playlist/get_add_to_playlist` once per video; click saves/removes through
  `browse/edit_playlist` (`WL`, `ACTION_ADD_VIDEO` / `ACTION_REMOVE_VIDEO_BY_VIDEO_ID`) with your session.
  States: idle → saving → saved (YouTube's bookmark, filled), or error with YouTube's reason ("Couldn't save to Watch later:
  Sign in required (401). Click to retry."), also sent to the debug center as an incident. Cards on the
  Watch later playlist are known saved. Keyboard: Tab to it, Enter/Space. The bookmark fills on the click
  (optimistic, class `on` while the request runs) and empties at once on a remove; a press squeezes it and it
  springs back to size.
- **Filters** (`#ytc-filters` on Home and Search; hover opens after 40 ms, 180 ms departure grace; keyboard
  focus or a click opens it too): minimum length Any/10+/20+/30+/60+ or a slider to 180 min (Home + Search,
  known durations only; live and unknown stay), **Shorts** (default off: hidden everywhere exactly as before;
  on: Shorts from Home, Search and related become cards with a "Shorts" badge in place of the duration and
  open our normal watch page; they pass the other filters but are kept at any minimum length; other pages never
  show them; hidden Shorts don't count toward preloading or chunks), Music (shows/hides videos YouTube labels "Music"/"Music
  video"; default hidden), Home only: Relevance/Popularity (sorts everything loaded, on screen and preloaded,
  by view count; unknown counts last; Relevance restores YouTube's order), Live (Home + related; Search keeps
  YouTube's own), Playlists only (with a Show all reset). Everything applies to parsed items at once; no
  request is ever made for a filter. A note says how many loaded cards are hidden. If four chunks in a row
  are hidden entirely, scrolling stops fetching and offers "Load more".
- **Home topic row** (one row: All · YouTube's own chips · your topics · + Topic). **YouTube's chips** are the
  Home response's feed filter chips (signed in, e.g. Gaming, Podcasts); a click loads that chip's feed through
  the chip's own continuation token (the next chip uses the token from the current response's bar), All
  restores the native feed from memory with no request. The + Topic editor lists them with earlier/later,
  Hide/Show and **rename** (`yourTubeNativeTopicsV1`): each name is an inline field (it reads as text until
  hovered or focused); Enter saves and the chip shows the new name at once, Escape (or leaving the field)
  cancels, an empty name restores YouTube's label. Only a renamed chip stores a `name` (≤ 40 characters), so
  YouTube's own label keeps showing for the rest. **Your topics** (1 Nov 2026: fast and fresh, following the
  pattern YouTube uses for its own topics): one anonymous `search` (hl=en) for the topic with YouTube's own filter params
  for This month + Videos gives the first screen; This year and any date (and their continuations) enrich in
  the background, fresher tiers always first; lexical scoring (Roblox topics need Roblox + tutorial evidence;
  RDC talks count), ≤ 3 per channel per batch; ≤ 30 requests and 160 cards per session; aborted on
  navigation. The old seed → related chain (≈ 15 s, old videos) is gone. Each topic chip has an × (hover /
  focus) that removes it. Chip and topic feeds stay in memory (no snapshot); back/forward returns to them, a
  fresh Home starts on All. The editor also lists/removes blocked websites.
- **Search filters** (Search page Filters panel, same hover-open/keyboard behavior): YouTube's own groups from
  the search response (Type, Duration, Upload date, Features: Live/4K/HD/Subtitles/Creative Commons/360°/VR180/
  3D/HDR/Location/Purchased, and Prioritize: Relevance/Popularity; YouTube no longer offers upload-date or
  rating sort). Each option is a link to the same query with the `params` YouTube returned (`sp=` in the URL,
  sent verbatim); hover prefetches it, nothing applies on hover, a click re-queries (instantly when
  prefetched), keyboard activation keeps focus on the option. YouTube's Duration options and the shared
  minimum length are one section. Active filters are chips beside Filters; one click removes one: YouTube
  sends selected options without an endpoint, so the removal is the current `params` minus that option's
  protobuf field (the field learned from the response's own options; bytes otherwise unchanged; matches
  YouTube's own params in the recorded cases); unreadable params offer one "Filters ×" chip (plain search).
  The Shorts type option is not offered.
- **Watch: playlist panel, Up next, Autoplay**: with `list=`, `next` is asked with the `playlistId`; the
  panel (title, playlist owner, current index / total or Mix, current row marked and scrolled into view, collapsible;
  no Loop/Shuffle) sits above **Up next** with the **Autoplay** switch. The app owns what's next
  (`App::next_video()`): the playlist's next item (always), else YouTube's autoplay target when Autoplay is
  on (never wrapping to an earlier playlist item). The player only reports `Out::Ended`: playlist items
  follow at once, plain autoplay counts down YouTube's `countDownSecs` with Play now / Cancel (player input
  cancels). With ≤ 45 s left the next video's `next` + VISIONOS are prefetched once.
- **Speed without waiting** (1 Nov 2026): video links prefetch `next` (+ VISIONOS) at once on hover,
  pointerdown and keyboard focus; in idle time the first 8 video cards in view are prefetched speculatively
  (low priority, two at a time; ≤ 30 speculative VISIONOS calls per 10 min because volume risks the bot
  check), cancelled on navigation except the page being opened; watch data and VISIONOS caches hold 40.
  Home and Subscriptions are refreshed in the background (first page + first continuation, low priority)
  when older than 2 min while the tab is visible and was used in the last 5 min; a click paints the snapshot
  and the fresh page in the same task, and a snapshot younger than 2 min skips the slow refresh. e2e speed
  table (Home first page +1.5 s, `next` +300 ms, before → after): Home click → fresh Home 1783 → 16 ms; idle
  results, click a card in view → related visible 437 → 13 ms; focus a card, Enter 150 ms later → 368 →
  162 ms.
- **Speed telemetry** (`kind: "perf"`, `schema: 2` in the debug center): every user-visible path as
  structured events, batched; see "Speed telemetry" below and the dashboard at http://127.0.0.1:47321/perf.
- **Perf reports** (`kind: "perf"` in the debug center): per-endpoint request → response ms (n/p50/p90/max),
  parse+render ms, navigation → cards/related visible, Home fresh, cache hit rates, errors per endpoint.
  Numbers and endpoint names only (no URLs, ids, tokens or cookies). At most every 10 min with new samples,
  on tab hide (≥ 1 min apart) and when the document is left.
- **Blocked websites**: beside every external description link a "Block example.com" control appears on
  hover/focus; "Blocked · Undo" reverses it. **Hold-back:** while at least one domain is blocked, cards of
  a newly appended batch without a verdict are hidden in the same task as the append (class `fxh`) until
  their own check answers (blocked → removed, allowed/unknown → shown); held cards jump the check queue
  (still 2 at a time) and the next chunk is checked ahead of scrolling. After a page load, batches appended
  before the rules arrive from storage wait for them. The first paint is never held. If no check answers
  for 4 s, held cards are shown unchecked. Cost (e2e, 150 ms checks, scrolling straight through 150 cards):
  held cards waited 6.5 s on average, 12.9 s at most, all below the viewport; with normal scrolling the
  look-ahead usually has the verdict ready. The debug overlay line `websites ·` shows the counts. Domains are normalized (no `www.`, lower case, YouTube/Google
  redirects unwrapped, shorteners not resolved); a rule covers subdomains, never lookalikes. While at least
  one domain is blocked, rendered Home/Search/related/topic cards are checked against their own description
  (anonymous `next` with a field mask, 2 at a time, once per video; descriptions already fetched by hover
  or a visit are reused) and hidden when they link to a blocked domain. Unknown stays visible.
- **Search history** in the search box dropdown (with YouTube's suggestions below it): recent searches on
  focus, prefix/substring matches while typing; ↑/↓ select (the box shows the row), Enter searches, Delete
  removes the selected history row, Escape closes; × removes with the mouse (focus stays in the box).
- **Resume and progress bars**: the player reports its position on pause, at the end and every 10 s (and
  when leaving). Opening a video without `t=` resumes there (not when finished or < 5 s in); cards show
  the local bar (a faint full track under a rewatch of a finished video); YouTube's bar elsewhere.
- Captions on/off and caption placement stay with the player (`p` in `ytcConfigV1`).

## Tests

- `cargo test --locked`: parsers, escaping, routes, auth, the player's pure parts (formats, WebM Cues / MP4 `sidx`,
  scheduler, captions, storyboards, live HLS + fMP4), features, actions, voice state machine, motion tokens (kept in
  sync with `client.css`), on recorded signed-out InnerTube responses in `tests/fixtures/` (IP and visitor data
  replaced with documentation values).
- `tests/e2e.cjs`: the real unpacked extension in Chromium with every request stubbed (YouTube-like CSP incl. Trusted
  Types): pages, navigation, escaping of hostile data, DNR rules, native-page cleanup, the own player on locally
  encoded media (`make-media.cjs`, `make-live.cjs`: WebM VP9/Opus and H.264/AAC fMP4 made by WebCodecs), failures and
  stalls, actions, features, voice search against a local WebSocket stand-in (`ws-mock.cjs`), SponsorBlock, motion
  and pixel checks, speed timings. Nothing reaches the network.
- `tests/live-*.cjs`: a few real requests against live YouTube / Deepgram, run by hand and sparingly.
- `tools/bench/video.cjs`: the live video-path benchmark (development build, signed out, real youtube.com
  and googlevideo, by hand): first frame, ≥ 1080p, seek and quality-switch times from the client's own
  telemetry for cold loads, SPA clicks with and without hover, and slow YouTube (a drip-feeding googlevideo
  proxy 10×/100× slower per node, per URL or for everything; a hung player request). Several builds are
  interleaved per round (`--ext a,b`): live timings drift ±30 % within a quarter of an hour.

## Known gaps / next steps

- Player UI: the CC popup's **Audio tab and the audio-language switch are unit-tested only** (the e2e
  media has one audio track); try a dubbed video.
  The `/feed/playlists` page (`FEplaylist_aggregation`)
  is parsed by the generic walker and untested signed in.
- Actions not built: writing comments/replies, rating comments, the subscription bell (notification
  level), playlist privacy choice on create (always Private), "Save" for playlists. No live chat or channel
  tabs other than Videos.
- Player: VISIONOS is one unofficial client profile (it can break whenever YouTube changes it); no daily
  canary yet; no live chat or live captions (HLS WebVTT), no low-latency live tuning (1 s segments work
but use the same rules), Shorts-style looping, storyboard thumbnails on the seek bar,
  audio-track (dub) choice, or caption styling/translation; MP4 (`sidx`) playback is unit-tested but
  the e2e media is WebM only; ABR is a simple throughput rule; volume keys ↑/↓ are not bound.
- Bundle size: the player adds ~171 KB of Wasm (297 → 468 KB; content.js 417 → 657 KB).
- Bundle size 0.3.0: the speed work adds ~45 KB of Wasm at the old `opt-level = 3`; with `"z"` the
  whole Wasm is 393 KB (content.js 562 KB) vs 513 KB / 722 KB.
- Timing table: `node tests/e2e.cjs` prints Home timings with 150 ms simulated InnerTube
  latency; `YTC_EXT=<another extension folder> YTC_TIMING_ONLY=1` runs the same table on another build.
- Since the `/s/desktop` DNR block was removed (0.2.1), the stopped YouTube document's bundles can still
  be *downloaded* (not run) when its first bytes arrive before `window.stop()`; the e2e check "site
  bundles never requested" fails about 1 run in 4, on `main`'s build as well.
- Suggestions come from YouTube's `/complete/search` with your cookies (like YouTube's own box).
- The YouTube HTML document is still requested (and stopped at the first bytes); the shell appears at
  roughly time-to-first-byte of that document.
- After `window.stop()` the document's `load` event does not fire (readyState is `complete`); no effect
  seen, but noted.

